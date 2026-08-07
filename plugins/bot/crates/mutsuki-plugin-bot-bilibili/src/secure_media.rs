//! Manual-redirect media fetch with a hard streamed size limit.

use std::io::Read;

use reqwest::blocking::{Client, Response};
use reqwest::header::LOCATION;
use reqwest::redirect::Policy;
use url::Url;

use super::BilibiliError;

pub(crate) const MEDIA_MAX_REDIRECTS: u8 = 5;
const READ_CHUNK: usize = 8 * 1024;

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

/// Download body with hop-by-hop allowlist and a hard streamed byte budget.
pub(crate) fn secure_media_download(
    client: &Client,
    url: &str,
    max_bytes: usize,
    allow: impl Fn(&Url) -> Result<(), BilibiliError>,
) -> Result<Vec<u8>, BilibiliError> {
    let start = Url::parse(url).map_err(|error| BilibiliError::DomainDenied(error.to_string()))?;
    let (_final_url, mut response) = follow_get(client, start, MEDIA_MAX_REDIRECTS, &allow)?;
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(BilibiliError::InvalidResponse(
            "media exceeds configured limit".into(),
        ));
    }
    read_body_with_limit(&mut response, max_bytes)
}

/// Follow redirects only (e.g. `b23.tv`), revalidating each hop.
pub(crate) fn secure_resolve_redirect(
    client: &Client,
    url: Url,
    allow: impl Fn(&Url) -> Result<(), BilibiliError>,
) -> Result<Url, BilibiliError> {
    let (final_url, _response) = follow_get(client, url, MEDIA_MAX_REDIRECTS, &allow)?;
    Ok(final_url)
}

fn follow_get(
    client: &Client,
    mut url: Url,
    max_redirects: u8,
    allow: &impl Fn(&Url) -> Result<(), BilibiliError>,
) -> Result<(Url, Response), BilibiliError> {
    for hop in 0..=max_redirects {
        allow(&url)?;
        let response = client
            .get(url.as_str())
            .send()
            .map_err(|error| BilibiliError::Transport(error.to_string()))?;
        if response.status().is_redirection() {
            if hop == max_redirects {
                return Err(BilibiliError::InvalidResponse("too many redirects".into()));
            }
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| {
                    BilibiliError::InvalidResponse("redirect is missing Location".into())
                })?;
            url = url
                .join(location)
                .map_err(|error| BilibiliError::InvalidResponse(error.to_string()))?;
            continue;
        }
        if !response.status().is_success() {
            return Err(BilibiliError::Transport(format!(
                "HTTP {}",
                response.status().as_u16()
            )));
        }
        allow(&url)?;
        return Ok((url, response));
    }
    Err(BilibiliError::InvalidResponse("too many redirects".into()))
}

fn read_body_with_limit(
    response: &mut Response,
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
    use std::thread;
    use std::time::Duration;

    use super::*;
    use crate::allow_bilibili_url;

    fn allow_loopback(url: &Url) -> Result<(), BilibiliError> {
        let host = url.host_str().unwrap_or_default();
        if url.scheme() == "http" && host == "127.0.0.1" {
            Ok(())
        } else {
            Err(BilibiliError::DomainDenied(host.into()))
        }
    }

    fn client() -> Client {
        try_media_client(Duration::from_secs(2), "mutsuki-test").unwrap()
    }

    fn serve(responses: Vec<Vec<u8>>) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let join = thread::spawn(move || {
            for scripted in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buf = [0_u8; 4096];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(&scripted);
            }
        });
        (format!("http://{address}"), join)
    }

    fn response(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
        let mut out =
            format!("HTTP/1.1 {status}\r\nConnection: close\r\n{headers}\r\n").into_bytes();
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn production_allowlist_rejects_http_and_foreign_hosts() {
        assert!(allow_bilibili_url(&Url::parse("http://i0.hdslb.com/x").unwrap()).is_err());
        assert!(allow_bilibili_url(&Url::parse("https://evil.example/x").unwrap()).is_err());
        assert!(allow_bilibili_url(&Url::parse("https://i0.hdslb.com/x").unwrap()).is_ok());
    }

    #[test]
    fn cross_domain_redirect_is_denied() {
        let (base, join) = serve(vec![response(
            "302 Found",
            "Location: http://evil.example/steal\r\n",
            b"",
        )]);
        let error =
            secure_media_download(&client(), &format!("{base}/start"), 1024, allow_loopback)
                .unwrap_err();
        join.join().unwrap();
        assert!(matches!(error, BilibiliError::DomainDenied(_)));
    }

    #[test]
    fn content_length_and_stream_limits_are_enforced() {
        let (base, join) = serve(vec![response(
            "200 OK",
            "Content-Length: 64\r\n",
            &vec![b'x'; 64],
        )]);
        let error = secure_media_download(&client(), &format!("{base}/cl"), 16, allow_loopback)
            .unwrap_err();
        join.join().unwrap();
        assert!(matches!(
            error,
            BilibiliError::InvalidResponse(message) if message.contains("limit")
        ));

        let (base, join) = serve(vec![response("200 OK", "", &vec![b'y'; 80])]);
        // No Content-Length: stream must stop at max_bytes.
        let error = secure_media_download(&client(), &format!("{base}/stream"), 32, allow_loopback)
            .unwrap_err();
        join.join().unwrap();
        assert!(matches!(
            error,
            BilibiliError::InvalidResponse(message) if message.contains("limit")
        ));
    }

    #[test]
    fn small_body_downloads_and_redirect_budget_is_capped() {
        let (base, join) = serve(vec![response(
            "200 OK",
            "Content-Length: 9\r\n",
            b"png-bytes",
        )]);
        let bytes =
            secure_media_download(&client(), &format!("{base}/ok"), 1024, allow_loopback).unwrap();
        join.join().unwrap();
        assert_eq!(bytes, b"png-bytes");

        // hop budget 1 => two GETs then reject; serve exactly two 302s.
        let (base, join) = serve(vec![
            response("302 Found", "Location: /h1\r\n", b""),
            response("302 Found", "Location: /h2\r\n", b""),
        ]);
        let start = Url::parse(&format!("{base}/h0")).unwrap();
        let error = follow_get(&client(), start, 1, &allow_loopback).unwrap_err();
        join.join().unwrap();
        assert!(matches!(
            error,
            BilibiliError::InvalidResponse(message) if message.contains("redirect")
        ));
    }
}
