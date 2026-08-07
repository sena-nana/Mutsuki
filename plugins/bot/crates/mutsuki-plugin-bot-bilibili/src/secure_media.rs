//! Allowlisted media download with manual redirects and a hard streamed size limit.
//!
//! Callers supply domain policy; this module never follows redirects blindly or buffers
//! an unbounded response body before enforcing `max_bytes`.

use std::io::Read;

use reqwest::blocking::Client;
use reqwest::header::LOCATION;
use reqwest::redirect::Policy;
use url::Url;

use super::BilibiliError;

pub(crate) const MEDIA_MAX_REDIRECTS: u8 = 5;
const READ_CHUNK: usize = 8 * 1024;

/// Builds a blocking client that never auto-follows redirects (callers revalidate each hop).
pub(crate) fn try_media_client(
    timeout: std::time::Duration,
    user_agent: &str,
) -> Result<Client, BilibiliError> {
    Client::builder()
        .timeout(timeout)
        .user_agent(user_agent)
        .redirect(Policy::none())
        .build()
        .map_err(|error| BilibiliError::Transport(error.to_string()))
}

/// Downloads `url` with hop-by-hop allowlist checks and a hard streamed byte budget.
pub(crate) fn secure_media_download(
    client: &Client,
    url: &str,
    max_bytes: usize,
    max_redirects: u8,
    allow: impl Fn(&Url) -> Result<(), BilibiliError>,
) -> Result<Vec<u8>, BilibiliError> {
    let mut current =
        Url::parse(url).map_err(|error| BilibiliError::DomainDenied(error.to_string()))?;
    allow(&current)?;

    for redirects_followed in 0..=max_redirects {
        allow(&current)?;
        let mut response = client
            .get(current.as_str())
            .send()
            .map_err(|error| BilibiliError::Transport(error.to_string()))?;
        let status = response.status();
        if status.is_redirection() {
            if redirects_followed == max_redirects {
                return Err(BilibiliError::InvalidResponse(
                    "too many media redirects".into(),
                ));
            }
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| {
                    BilibiliError::InvalidResponse("redirect is missing Location".into())
                })?;
            current = current
                .join(location)
                .map_err(|error| BilibiliError::InvalidResponse(error.to_string()))?;
            // Drop the redirect body without buffering it into the caller budget.
            drop(response);
            continue;
        }
        if !status.is_success() {
            return Err(BilibiliError::Transport(format!(
                "media download returned status {}",
                status.as_u16()
            )));
        }
        if let Some(length) = response.content_length()
            && length > max_bytes as u64
        {
            return Err(BilibiliError::InvalidResponse(
                "media exceeds configured limit".into(),
            ));
        }
        // Final hop must still satisfy policy (covers any host rewrite via redirects).
        allow(&current)?;
        return read_body_with_limit(&mut response, max_bytes);
    }

    Err(BilibiliError::InvalidResponse(
        "too many media redirects".into(),
    ))
}

/// Follows redirects for non-body resolution (e.g. `b23.tv`) with the same hop policy.
pub(crate) fn secure_resolve_redirect(
    client: &Client,
    url: Url,
    max_redirects: u8,
    allow: impl Fn(&Url) -> Result<(), BilibiliError>,
) -> Result<Url, BilibiliError> {
    let mut current = url;
    allow(&current)?;
    for redirects_followed in 0..=max_redirects {
        allow(&current)?;
        let response = client
            .get(current.as_str())
            .send()
            .map_err(|error| BilibiliError::Transport(error.to_string()))?;
        let status = response.status();
        if status.is_redirection() {
            if redirects_followed == max_redirects {
                return Err(BilibiliError::InvalidResponse(
                    "too many resolve redirects".into(),
                ));
            }
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| {
                    BilibiliError::InvalidResponse("redirect is missing Location".into())
                })?;
            current = current
                .join(location)
                .map_err(|error| BilibiliError::InvalidResponse(error.to_string()))?;
            drop(response);
            continue;
        }
        // Some short-link endpoints answer 200 on the final resource; keep the request URL.
        allow(&current)?;
        return Ok(current);
    }
    Err(BilibiliError::InvalidResponse(
        "too many resolve redirects".into(),
    ))
}

fn read_body_with_limit(
    response: &mut reqwest::blocking::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, BilibiliError> {
    let mut body = Vec::new();
    let mut buffer = [0_u8; READ_CHUNK];
    loop {
        let read = response
            .read(&mut buffer)
            .map_err(|error| BilibiliError::Transport(error.to_string()))?;
        if read == 0 {
            break;
        }
        let next = body.len().checked_add(read).ok_or_else(|| {
            BilibiliError::InvalidResponse("media exceeds configured limit".into())
        })?;
        if next > max_bytes {
            return Err(BilibiliError::InvalidResponse(
                "media exceeds configured limit".into(),
            ));
        }
        body.extend_from_slice(&buffer[..read]);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    use super::*;

    fn allow_test_host(url: &Url) -> Result<(), BilibiliError> {
        let host = url.host_str().unwrap_or_default();
        if url.scheme() == "http" && (host == "127.0.0.1" || host == "localhost") {
            Ok(())
        } else {
            Err(BilibiliError::DomainDenied(host.into()))
        }
    }

    fn allow_only_origin(url: &Url, origin_host: &str) -> Result<(), BilibiliError> {
        let host = url.host_str().unwrap_or_default();
        if url.scheme() == "http" && host == origin_host {
            Ok(())
        } else {
            Err(BilibiliError::DomainDenied(host.into()))
        }
    }

    struct MockServer {
        base: String,
        hits: Arc<Mutex<usize>>,
        _join: thread::JoinHandle<()>,
    }

    fn spawn_scripted(responses: Vec<Vec<u8>>) -> MockServer {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let hits = Arc::new(Mutex::new(0_usize));
        let hits_thread = hits.clone();
        let join = thread::spawn(move || {
            for scripted in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0_u8; 4096];
                let _ = stream.read(&mut buffer);
                *hits_thread.lock().unwrap() += 1;
                let _ = stream.write_all(&scripted);
                let _ = stream.flush();
            }
        });
        MockServer {
            base: format!("http://{address}"),
            hits,
            _join: join,
        }
    }

    fn http_response(status: &str, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
        let mut message = format!("HTTP/1.1 {status}\r\nConnection: close\r\n");
        let mut has_length = false;
        for (name, value) in headers {
            if name.eq_ignore_ascii_case("Content-Length") {
                has_length = true;
            }
            message.push_str(&format!("{name}: {value}\r\n"));
        }
        if !has_length {
            message.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        message.push_str("\r\n");
        let mut bytes = message.into_bytes();
        bytes.extend_from_slice(body);
        bytes
    }

    fn client() -> Client {
        try_media_client(Duration::from_secs(2), "mutsuki-test").unwrap()
    }

    #[test]
    fn cross_domain_redirect_is_denied_before_reading_body() {
        let server = spawn_scripted(vec![http_response(
            "302 Found",
            &[("Location", "http://evil.example/steal")],
            b"",
        )]);
        let error = secure_media_download(
            &client(),
            &format!("{}/start", server.base),
            1024,
            MEDIA_MAX_REDIRECTS,
            |url| allow_only_origin(url, "127.0.0.1"),
        )
        .unwrap_err();
        assert!(matches!(error, BilibiliError::DomainDenied(_)));
        assert_eq!(*server.hits.lock().unwrap(), 1);
    }

    #[test]
    fn http_scheme_is_denied_for_production_policy() {
        let error = secure_media_download(
            &client(),
            "http://i0.hdslb.com/bfs/x.jpg",
            1024,
            MEDIA_MAX_REDIRECTS,
            |url| {
                // Mirror production: HTTPS + bilibili hosts only.
                let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
                let allowed = host == "bilibili.com"
                    || host.ends_with(".bilibili.com")
                    || host == "hdslb.com"
                    || host.ends_with(".hdslb.com")
                    || host == "b23.tv";
                if url.scheme() == "https" && allowed && url.username().is_empty() {
                    Ok(())
                } else {
                    Err(BilibiliError::DomainDenied(host.into()))
                }
            },
        )
        .unwrap_err();
        assert!(matches!(error, BilibiliError::DomainDenied(_)));
    }

    #[test]
    fn content_length_over_limit_is_rejected() {
        let server = spawn_scripted(vec![http_response(
            "200 OK",
            &[("Content-Length", "64")],
            &vec![b'x'; 64],
        )]);
        let error = secure_media_download(
            &client(),
            &format!("{}/big", server.base),
            16,
            MEDIA_MAX_REDIRECTS,
            allow_test_host,
        )
        .unwrap_err();
        assert!(
            matches!(error, BilibiliError::InvalidResponse(message) if message.contains("limit"))
        );
    }

    #[test]
    fn streamed_body_stops_when_max_bytes_is_crossed() {
        // Omit Content-Length so only the stream path enforces the budget.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let join = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0_u8; 4096];
            let _ = stream.read(&mut buffer);
            let header = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n";
            let _ = stream.write_all(header);
            let _ = stream.write_all(&vec![b'y'; 40]);
            let _ = stream.write_all(&vec![b'z'; 40]);
            let _ = stream.flush();
        });
        let error = secure_media_download(
            &client(),
            &format!("http://{address}/chunked"),
            48,
            MEDIA_MAX_REDIRECTS,
            allow_test_host,
        )
        .unwrap_err();
        join.join().unwrap();
        assert!(
            matches!(error, BilibiliError::InvalidResponse(message) if message.contains("limit"))
        );
    }

    #[test]
    fn redirect_loop_exceeding_max_is_rejected() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let base = format!("http://{address}");
        let server_base = base.clone();
        let join = thread::spawn(move || {
            // Client with max_redirects=2 issues three GETs (hops 0..=2) then rejects.
            for index in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0_u8; 4096];
                let _ = stream.read(&mut buffer);
                let location = format!("{server_base}/hop-{}", index + 1);
                let response = http_response("302 Found", &[("Location", &location)], b"");
                let _ = stream.write_all(&response);
            }
        });
        let error = secure_media_download(
            &client(),
            &format!("{base}/hop-0"),
            1024,
            2,
            allow_test_host,
        )
        .unwrap_err();
        join.join().unwrap();
        assert!(
            matches!(error, BilibiliError::InvalidResponse(message) if message.contains("redirect"))
        );
    }

    #[test]
    fn small_body_downloads_successfully() {
        let server = spawn_scripted(vec![http_response("200 OK", &[], b"png-bytes")]);
        let bytes = secure_media_download(
            &client(),
            &format!("{}/ok", server.base),
            1024,
            MEDIA_MAX_REDIRECTS,
            allow_test_host,
        )
        .unwrap();
        assert_eq!(bytes, b"png-bytes");
    }
}
