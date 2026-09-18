//! Allowlisted HTTP fetch with manual redirects and a hard streamed size budget.
//!
//! The Bilibili and Workshop plugins each grew their own copy of this loop, down to
//! the same redirect cap and chunk size. Two copies of an SSRF guard is one too many:
//! a hop-validation fix applied to one silently leaves the other exposed. Neither can
//! depend on the other (plugin crates may not depend on plugin crates), so the
//! primitive lives here and both consume it.
//!
//! Callers that reach HTTP from inside a node runner should submit
//! `mutsuki.std.io.http` instead — the std plugin owns redirect, allowlist and budget
//! policy there. This exists for the synchronous transports that have no task
//! submitter and would otherwise hand-roll it again.

use std::io::Read;
use std::time::Duration;

use reqwest::blocking::{Client, Response};
use reqwest::header::LOCATION;
use reqwest::redirect::Policy;
use url::Url;

/// Redirect hops a media fetch may follow before it is refused.
pub const MEDIA_MAX_REDIRECTS: u8 = 5;
const READ_CHUNK: usize = 8 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum SecureFetchError {
    /// The allowlist refused this URL, or it could not be parsed.
    #[error("{0}")]
    Denied(String),
    /// Transport failure, or a non-success status.
    #[error("{0}")]
    Transport(String),
    /// The response was unusable: too many redirects, missing `Location`, oversized.
    #[error("{0}")]
    InvalidResponse(String),
}

/// Builds a client that performs **no** automatic redirects.
///
/// Automatic redirects would bypass the per-hop allowlist, which is the whole point
/// of this module, so the policy is fixed here rather than left to callers.
///
/// # Errors
///
/// Returns [`SecureFetchError::Transport`] when the client cannot be built.
pub fn media_client(timeout: Duration, user_agent: &str) -> Result<Client, SecureFetchError> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    Client::builder()
        .timeout(timeout)
        .user_agent(user_agent)
        .redirect(Policy::none())
        .build()
        .map_err(|error| SecureFetchError::Transport(error.to_string()))
}

/// Follows redirects only, revalidating every hop, and returns the final URL.
///
/// # Errors
///
/// Returns an error when a hop is denied, the transport fails, or the redirect
/// budget is exhausted.
pub fn resolve_redirect(
    client: &Client,
    url: Url,
    allow: impl Fn(&Url) -> Result<(), SecureFetchError>,
) -> Result<Url, SecureFetchError> {
    let (final_url, _response) = follow_get(client, url, &allow)?;
    Ok(final_url)
}

/// Fetches a body with a hop-by-hop allowlist and a hard streamed byte budget.
///
/// The budget is enforced while reading rather than from `Content-Length` alone: a
/// server that understates or omits the header cannot make the caller buffer more
/// than `max_bytes`.
///
/// # Errors
///
/// Returns an error when a hop is denied, the transport fails, or the body exceeds
/// `max_bytes`.
pub fn download(
    client: &Client,
    url: &str,
    max_bytes: usize,
    allow: impl Fn(&Url) -> Result<(), SecureFetchError>,
) -> Result<(Url, Vec<u8>), SecureFetchError> {
    let start = Url::parse(url).map_err(|error| SecureFetchError::Denied(error.to_string()))?;
    let (final_url, mut response) = follow_get(client, start, &allow)?;
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(SecureFetchError::InvalidResponse(
            "response exceeds configured limit".into(),
        ));
    }
    let body = read_body_with_limit(&mut response, max_bytes)?;
    Ok((final_url, body))
}

fn follow_get(
    client: &Client,
    mut url: Url,
    allow: &impl Fn(&Url) -> Result<(), SecureFetchError>,
) -> Result<(Url, Response), SecureFetchError> {
    for hop in 0..=MEDIA_MAX_REDIRECTS {
        // Checked before every request, so a redirect cannot walk off the allowlist.
        allow(&url)?;
        let response = client
            .get(url.as_str())
            .send()
            .map_err(|error| SecureFetchError::Transport(error.to_string()))?;
        if response.status().is_redirection() {
            if hop == MEDIA_MAX_REDIRECTS {
                return Err(SecureFetchError::InvalidResponse(
                    "too many redirects".into(),
                ));
            }
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| {
                    SecureFetchError::InvalidResponse("redirect is missing Location".into())
                })?;
            url = url
                .join(location)
                .map_err(|error| SecureFetchError::InvalidResponse(error.to_string()))?;
            continue;
        }
        if !response.status().is_success() {
            return Err(SecureFetchError::Transport(format!(
                "HTTP {}",
                response.status().as_u16()
            )));
        }
        // Re-checked after the final hop resolves, so the URL that produced the body
        // is the one that was authorized.
        allow(&url)?;
        return Ok((url, response));
    }
    Err(SecureFetchError::InvalidResponse(
        "too many redirects".into(),
    ))
}

fn read_body_with_limit(
    response: &mut Response,
    max_bytes: usize,
) -> Result<Vec<u8>, SecureFetchError> {
    let mut body = Vec::new();
    let mut buffer = [0_u8; READ_CHUNK];
    loop {
        let read = response
            .read(&mut buffer)
            .map_err(|error| SecureFetchError::Transport(error.to_string()))?;
        if read == 0 {
            break;
        }
        let next = body.len().checked_add(read).ok_or_else(|| {
            SecureFetchError::InvalidResponse("response exceeds configured limit".into())
        })?;
        if next > max_bytes {
            return Err(SecureFetchError::InvalidResponse(
                "response exceeds configured limit".into(),
            ));
        }
        body.extend_from_slice(&buffer[..read]);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::net::TcpListener;
    use std::thread;

    use super::*;

    fn allow_loopback(url: &Url) -> Result<(), SecureFetchError> {
        let host = url.host_str().unwrap_or_default();
        if url.scheme() == "http" && host == "127.0.0.1" {
            Ok(())
        } else {
            Err(SecureFetchError::Denied(host.into()))
        }
    }

    fn client() -> Client {
        media_client(Duration::from_secs(2), "mutsuki-test").unwrap()
    }

    fn serve(responses: Vec<Vec<u8>>) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let join = thread::spawn(move || {
            for scripted in responses {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
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
    fn redirect_off_the_allowlist_is_denied() {
        // The reason redirects are followed by hand: an automatic policy would fetch
        // this without ever consulting the allowlist.
        let (base, join) = serve(vec![response(
            "302 Found",
            "Location: http://evil.example/steal\r\n",
            b"",
        )]);
        let error =
            download(&client(), &format!("{base}/start"), 1024, allow_loopback).unwrap_err();
        join.join().unwrap();
        assert!(matches!(error, SecureFetchError::Denied(_)), "{error:?}");
    }

    #[test]
    fn size_budget_holds_with_and_without_content_length() {
        let (base, join) = serve(vec![response(
            "200 OK",
            "Content-Length: 64\r\n",
            &[b'x'; 64],
        )]);
        let error = download(&client(), &format!("{base}/cl"), 16, allow_loopback).unwrap_err();
        join.join().unwrap();
        assert!(
            matches!(&error, SecureFetchError::InvalidResponse(m) if m.contains("limit")),
            "{error:?}"
        );

        // No Content-Length: a server that omits it must not be able to make the
        // caller buffer past the budget, so the read loop enforces it too.
        let (base, join) = serve(vec![response("200 OK", "", &[b'y'; 80])]);
        let error = download(&client(), &format!("{base}/stream"), 32, allow_loopback).unwrap_err();
        join.join().unwrap();
        assert!(
            matches!(&error, SecureFetchError::InvalidResponse(m) if m.contains("limit")),
            "{error:?}"
        );
    }

    #[test]
    fn allowed_body_downloads_and_reports_its_final_url() {
        let (base, join) = serve(vec![response(
            "200 OK",
            "Content-Length: 9\r\n",
            b"png-bytes",
        )]);
        let (final_url, bytes) =
            download(&client(), &format!("{base}/ok"), 1024, allow_loopback).unwrap();
        join.join().unwrap();
        assert_eq!(bytes, b"png-bytes");
        assert!(final_url.as_str().ends_with("/ok"));
    }

    #[test]
    fn redirect_budget_is_capped() {
        // One more hop than the budget allows, all on the allowlist, so the refusal
        // can only come from the cap itself.
        let hops = usize::from(MEDIA_MAX_REDIRECTS) + 1;
        let (base, join) = serve(
            (0..hops)
                .map(|n| response("302 Found", &format!("Location: /h{n}\r\n"), b""))
                .collect(),
        );
        let error = download(&client(), &format!("{base}/h"), 1024, allow_loopback).unwrap_err();
        join.join().unwrap();
        assert!(
            matches!(&error, SecureFetchError::InvalidResponse(m) if m.contains("redirect")),
            "{error:?}"
        );
    }
}
