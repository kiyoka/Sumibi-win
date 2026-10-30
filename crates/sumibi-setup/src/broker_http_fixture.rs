//! Disposable loopback-only endpoint for the explicit broker/TSF diagnostic.
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

type Failure = Box<dyn std::error::Error>;

pub(crate) struct Fixture {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<Result<(), String>>>,
    port: u16,
}
impl Fixture {
    pub(crate) fn start() -> Result<Self, Failure> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let thread = std::thread::spawn(move || -> Result<(), String> {
            let deadline = Instant::now() + Duration::from_secs(30);
            while !thread_stop.load(Ordering::Acquire) && Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, address)) => {
                        if !address.ip().is_loopback() {
                            return Err("non-loopback peer".into());
                        }
                        stream
                            .set_read_timeout(Some(Duration::from_secs(3)))
                            .map_err(|_| "read timeout setup")?;
                        stream
                            .set_write_timeout(Some(Duration::from_secs(3)))
                            .map_err(|_| "write timeout setup")?;
                        return serve(&mut stream);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2))
                    }
                    Err(_) => return Err("accept failed".into()),
                }
            }
            if thread_stop.load(Ordering::Acquire) {
                Ok(())
            } else {
                Err("fixture expired".into())
            }
        });
        Ok(Self {
            stop,
            thread: Some(thread),
            port,
        })
    }
    pub(crate) fn port(&self) -> u16 {
        self.port
    }
    pub(crate) fn finish(mut self) -> Result<(), Failure> {
        self.stop.store(true, Ordering::Release);
        self.thread
            .take()
            .ok_or("fixture already finished")?
            .join()
            .map_err(|_| "fixture panicked")?
            .map_err(|error| error.into())
    }
}
fn serve(stream: &mut impl ReadWrite) -> Result<(), String> {
    let mut request = Vec::new();
    let (header_end, length) = loop {
        if request.len() > 8192 {
            return Err("request too large".into());
        }
        if let Some(position) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            let end = position + 4;
            let headers = std::str::from_utf8(&request[..end]).map_err(|_| "invalid headers")?;
            if !headers.starts_with("POST /convert HTTP/1.1\r\n")
                || !headers
                    .lines()
                    .any(|line| line == "Authorization: Bearer fixture")
            {
                return Err("unexpected request headers".into());
            }
            let values: Vec<_> = headers
                .lines()
                .filter_map(|line| line.strip_prefix("Content-Length: "))
                .collect();
            if values.len() != 1 {
                return Err("invalid length".into());
            }
            let length: usize = values[0].parse().map_err(|_| "invalid length")?;
            if length > 4096 {
                return Err("request body too large".into());
            }
            break (end, length);
        }
        let mut bytes = [0; 1024];
        let read = stream.read(&mut bytes).map_err(|_| "request read failed")?;
        if read == 0 {
            return Err("truncated request".into());
        }
        request.extend_from_slice(&bytes[..read]);
    };
    while request.len() < header_end + length {
        let mut bytes = [0; 1024];
        let read = stream
            .read(&mut bytes)
            .map_err(|_| "request body read failed")?;
        if read == 0 {
            return Err("truncated request body".into());
        }
        request.extend_from_slice(&bytes[..read]);
    }
    if request.len() != header_end + length {
        return Err("extra request bytes".into());
    }
    let body: serde_json::Value =
        serde_json::from_slice(&request[header_end..]).map_err(|_| "invalid request body")?;
    if body != serde_json::json!({"text": "nihongo"}) {
        return Err("unexpected fixture input".into());
    }
    let response = serde_json::to_vec(&serde_json::json!({"text": "日本語"}))
        .map_err(|_| "response encoding")?;
    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", response.len()).map_err(|_| "response header failed")?;
    stream
        .write_all(&response)
        .map_err(|_| "response body failed")?;
    Ok(())
}

trait ReadWrite: Read + Write {}
impl<T: Read + Write> ReadWrite for T {}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Cursor};

    struct Exchange {
        input: Cursor<Vec<u8>>,
        output: Vec<u8>,
        chunk: usize,
    }
    impl Read for Exchange {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            let size = bytes.len().min(self.chunk);
            self.input.read(&mut bytes[..size])
        }
    }
    impl Write for Exchange {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.output.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    fn exchange(body: &str, auth: &str, extra: &str) -> Exchange {
        let request = format!("POST /convert HTTP/1.1\r\nAuthorization: Bearer {auth}\r\nContent-Length: {}\r\n\r\n{body}{extra}", body.len());
        Exchange {
            input: Cursor::new(request.into_bytes()),
            output: Vec::new(),
            chunk: 7,
        }
    }
    #[test]
    fn loopback_fixture_validates_fragmented_request_and_emits_json() {
        let mut io = exchange(r#"{"text":"nihongo"}"#, "fixture", "");
        serve(&mut io).unwrap();
        let response = std::str::from_utf8(&io.output).unwrap();
        let (header, body) = response.split_once("\r\n\r\n").unwrap();
        assert!(header.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(header.contains(&format!("Content-Length: {}", body.len())));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(body).unwrap(),
            serde_json::json!({"text":"日本語"})
        );
    }
    #[test]
    fn loopback_fixture_rejects_wrong_credentials_and_input() {
        for (body, auth) in [
            (r#"{"text":"nihongo"}"#, "wrong"),
            (r#"{"text":"other"}"#, "fixture"),
        ] {
            let mut io = exchange(body, auth, "");
            assert!(serve(&mut io).is_err());
            assert!(io.output.is_empty());
        }
    }
}
