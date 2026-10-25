//! Deliberately restricted HTTP/1.1 fixture. Only a numeric loopback address
//! (or the literal localhost mapped to 127.0.0.1) is contacted. This is not the
//! product's HTTPS/WinHTTP adapter or the external conversion API contract.
use crate::manual::{ConversionTransport, Destination, Scheme, TransportError};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

const MAX_HEADER_BYTES: usize = 8192;
const MAX_RESPONSE_BYTES: usize = 8192;

pub struct DiagnosticLoopbackHttp;

impl ConversionTransport for DiagnosticLoopbackHttp {
    fn convert(
        &self,
        destination: &Destination,
        credential: &str,
        source: &str,
        deadline: Instant,
    ) -> Result<String, TransportError> {
        if destination.scheme() != Scheme::HttpLoopback
            || !credential.bytes().all(|byte| byte.is_ascii_graphic())
            || credential.is_empty()
            || !sumibi_protocol::message::valid_text(source)
        {
            return Err(TransportError::InvalidRequest);
        }
        let address = if destination.host() == "localhost" {
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        } else {
            destination
                .host()
                .parse::<IpAddr>()
                .map_err(|_| TransportError::InvalidRequest)?
        };
        if !address.is_loopback() {
            return Err(TransportError::InvalidRequest);
        }
        let body = serde_json::to_vec(&json!({"text": source}))
            .map_err(|_| TransportError::InvalidRequest)?;
        let host_header = if address.is_ipv6() {
            format!("[{}]", destination.host())
        } else {
            destination.host().to_owned()
        };
        let headers = format!(
            "POST {} HTTP/1.1\r\nHost: {}:{}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            destination.base_path(),
            host_header,
            destination.port(),
            credential,
            body.len()
        );
        let address = SocketAddr::new(address, destination.port());
        let mut stream =
            TcpStream::connect_timeout(&address, remaining(deadline)?).map_err(classify_io)?;
        set_write_deadline(&stream, deadline)?;
        stream.write_all(headers.as_bytes()).map_err(classify_io)?;
        set_write_deadline(&stream, deadline)?;
        stream.write_all(&body).map_err(classify_io)?;

        let mut response = Vec::new();
        let header_end = loop {
            if let Some(end) = find_header_end(&response) {
                break end;
            }
            if response.len() >= MAX_HEADER_BYTES {
                return Err(TransportError::ResponseTooLarge);
            }
            let mut chunk = [0u8; 1024];
            set_read_deadline(&stream, deadline)?;
            let count = stream.read(&mut chunk).map_err(classify_io)?;
            if count == 0 {
                return Err(TransportError::InvalidResponse);
            }
            response.extend_from_slice(&chunk[..count]);
            if response.len() > MAX_HEADER_BYTES + MAX_RESPONSE_BYTES {
                return Err(TransportError::ResponseTooLarge);
            }
        };
        if header_end > MAX_HEADER_BYTES {
            return Err(TransportError::ResponseTooLarge);
        }
        let length = parse_headers(&response[..header_end])?;
        if response.len() - header_end > length {
            return Err(TransportError::InvalidResponse);
        }
        while response.len() - header_end < length {
            let mut chunk = [0u8; 1024];
            set_read_deadline(&stream, deadline)?;
            let count = stream.read(&mut chunk).map_err(classify_io)?;
            if count == 0 {
                return Err(TransportError::InvalidResponse);
            }
            if response.len() - header_end + count > length {
                return Err(TransportError::InvalidResponse);
            }
            response.extend_from_slice(&chunk[..count]);
        }
        remaining(deadline)?;
        let value: Value = serde_json::from_slice(&response[header_end..])
            .map_err(|_| TransportError::InvalidResponse)?;
        let object = value.as_object().ok_or(TransportError::InvalidResponse)?;
        if object.len() != 1 {
            return Err(TransportError::InvalidResponse);
        }
        object
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or(TransportError::InvalidResponse)
    }
}

fn remaining(deadline: Instant) -> Result<Duration, TransportError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        Err(TransportError::Timeout)
    } else {
        Ok(remaining)
    }
}
fn set_write_deadline(stream: &TcpStream, deadline: Instant) -> Result<(), TransportError> {
    stream
        .set_write_timeout(Some(remaining(deadline)?))
        .map_err(classify_io)
}
fn set_read_deadline(stream: &TcpStream, deadline: Instant) -> Result<(), TransportError> {
    stream
        .set_read_timeout(Some(remaining(deadline)?))
        .map_err(classify_io)
}
fn classify_io(error: std::io::Error) -> TransportError {
    match error.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => TransportError::Timeout,
        _ => TransportError::Connection,
    }
}
fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(4)
        .position(|part| part == b"\r\n\r\n")
        .map(|index| index + 4)
}
fn parse_headers(header: &[u8]) -> Result<usize, TransportError> {
    let header = std::str::from_utf8(header).map_err(|_| TransportError::InvalidResponse)?;
    let mut lines = header
        .strip_suffix("\r\n\r\n")
        .ok_or(TransportError::InvalidResponse)?
        .split("\r\n");
    let status = lines.next().ok_or(TransportError::InvalidResponse)?;
    let mut parts = status.splitn(3, ' ');
    if !matches!(parts.next(), Some("HTTP/1.1" | "HTTP/1.0")) {
        return Err(TransportError::InvalidResponse);
    }
    let code = parts.next().ok_or(TransportError::InvalidResponse)?;
    if code.len() != 3 || !code.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(TransportError::InvalidResponse);
    }
    let code: u16 = code.parse().map_err(|_| TransportError::InvalidResponse)?;
    match code {
        200 => {}
        300..=399 => return Err(TransportError::Redirect),
        401 | 403 => return Err(TransportError::Authentication),
        429 => return Err(TransportError::RateLimited),
        _ => return Err(TransportError::Http),
    }
    let mut length = None;
    let mut content_type = false;
    for line in lines {
        if line.is_empty() || line.contains(['\r', '\n']) || line.starts_with([' ', '\t']) {
            return Err(TransportError::InvalidResponse);
        }
        let (name, value) = line
            .split_once(':')
            .ok_or(TransportError::InvalidResponse)?;
        let value = value.trim_ascii();
        if name.eq_ignore_ascii_case("content-length") {
            if length.is_some() || value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                return Err(TransportError::InvalidResponse);
            }
            let parsed: usize = value
                .parse()
                .map_err(|_| TransportError::ResponseTooLarge)?;
            if parsed > MAX_RESPONSE_BYTES {
                return Err(TransportError::ResponseTooLarge);
            }
            length = Some(parsed);
        } else if name.eq_ignore_ascii_case("content-type") {
            if content_type || !value.eq_ignore_ascii_case("application/json") {
                return Err(TransportError::InvalidResponse);
            }
            content_type = true;
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(TransportError::InvalidResponse);
        }
    }
    if !content_type {
        return Err(TransportError::InvalidResponse);
    }
    length.ok_or(TransportError::InvalidResponse)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manual::{self, Credential, ManualRequest, SendError};
    use crate::policy::PolicyAuthority;
    use std::net::TcpListener;
    use std::thread;
    use sumibi_protocol::message::Revision;

    fn serve_once(
        response: Vec<u8>,
        delay: Duration,
    ) -> (Destination, thread::JoinHandle<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            let mut chunk = [0u8; 1024];
            let header_end = loop {
                let count = stream.read(&mut chunk).unwrap();
                assert!(count > 0);
                request.extend_from_slice(&chunk[..count]);
                if let Some(end) = find_header_end(&request) {
                    break end;
                }
            };
            let headers = std::str::from_utf8(&request[..header_end]).unwrap();
            let length: usize = headers
                .lines()
                .find_map(|line| line.strip_prefix("Content-Length: "))
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            while request.len() - header_end < length {
                let count = stream.read(&mut chunk).unwrap();
                assert!(count > 0);
                request.extend_from_slice(&chunk[..count]);
            }
            thread::sleep(delay);
            let _ = stream.write_all(&response);
            request
        });
        (
            Destination::new(Scheme::HttpLoopback, "127.0.0.1", port, "/convert").unwrap(),
            handle,
        )
    }

    fn send(destination: &Destination, deadline: Duration) -> Result<String, SendError> {
        let authority = PolicyAuthority::new(destination.clone());
        authority
            .replace_credential(Some(Credential::new("synthetic-key".into()).unwrap()))
            .unwrap();
        let version = authority.grant_manual_consent(destination).unwrap();
        manual::convert(
            ManualRequest {
                revision: Revision {
                    input: 1,
                    settings: version.settings,
                    authorization: version.authorization,
                },
                destination,
                source: "nihongo",
                deadline: Instant::now() + deadline,
            },
            &authority,
            &DiagnosticLoopbackHttp,
        )
    }

    fn response(status: &str, body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ).into_bytes()
    }

    #[test]
    fn sends_only_consented_loopback_request_and_parses_json() {
        let (destination, server) =
            serve_once(response("200 OK", r#"{"text":"日本語"}"#), Duration::ZERO);
        assert_eq!(
            send(&destination, Duration::from_secs(2)),
            Ok("日本語".into())
        );
        let request = server.join().unwrap();
        let text = std::str::from_utf8(&request).unwrap();
        assert!(text.starts_with("POST /convert HTTP/1.1\r\n"));
        assert!(text.contains(&format!("Host: 127.0.0.1:{}\r\n", destination.port())));
        assert!(text.contains("Authorization: Bearer synthetic-key\r\n"));
        assert!(text.contains("Content-Type: application/json\r\n"));
        assert!(text.ends_with("\r\n\r\n{\"text\":\"nihongo\"}"));
    }

    #[test]
    fn refuses_redirect_and_classifies_remote_failures() {
        for (status, expected) in [
            ("302 Found", TransportError::Redirect),
            ("401 Unauthorized", TransportError::Authentication),
            ("403 Forbidden", TransportError::Authentication),
            ("429 Too Many Requests", TransportError::RateLimited),
            ("500 Internal Server Error", TransportError::Http),
        ] {
            let (destination, server) = serve_once(response(status, ""), Duration::ZERO);
            assert_eq!(
                send(&destination, Duration::from_secs(2)),
                Err(SendError::Transport(expected))
            );
            server.join().unwrap();
        }
    }

    #[test]
    fn rejects_oversized_or_ambiguous_response() {
        let oversized =
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 8193\r\n\r\n"
                .to_vec();
        let (destination, server) = serve_once(oversized, Duration::ZERO);
        assert_eq!(
            send(&destination, Duration::from_secs(2)),
            Err(SendError::Transport(TransportError::ResponseTooLarge))
        );
        server.join().unwrap();

        let duplicate = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nContent-Length: 11\r\n\r\n".to_vec();
        let (destination, server) = serve_once(duplicate, Duration::ZERO);
        assert_eq!(
            send(&destination, Duration::from_secs(2)),
            Err(SendError::Transport(TransportError::InvalidResponse))
        );
        server.join().unwrap();
    }

    #[test]
    fn deadline_and_https_are_enforced() {
        let (destination, server) = serve_once(
            response("200 OK", r#"{"text":"日本語"}"#),
            Duration::from_millis(300),
        );
        assert_eq!(
            send(&destination, Duration::from_millis(100)),
            Err(SendError::Transport(TransportError::Timeout))
        );
        server.join().unwrap();

        let https = Destination::new(Scheme::Https, "127.0.0.1", 443, "/convert").unwrap();
        assert_eq!(
            DiagnosticLoopbackHttp.convert(
                &https,
                "synthetic-key",
                "nihongo",
                Instant::now() + Duration::from_secs(1)
            ),
            Err(TransportError::InvalidRequest)
        );
    }
}
