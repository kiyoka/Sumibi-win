//! Native HTTPS preparation, ordered sending and bounded response reception.
//! The complete conversion adapter remains separate work.
use crate::manual::{Destination, Scheme, SendError, SendGate, TransportError};
use std::time::Instant;
use sumibi_winhttp::{Error, PreparedRequest};

pub fn prepare(destination: &Destination) -> Result<PreparedRequest, Error> {
    if destination.scheme() != Scheme::Https {
        return Err(Error::InvalidDestination);
    }
    PreparedRequest::new(
        destination.host(),
        destination.port(),
        destination.base_path(),
    )
}

/// Hold the Host send guard until WinHTTP confirms all initial body bytes
/// were sent. The returned request still owns its buffers and needs response
/// reception or explicit close. No endpoint fallback or retry is performed.
pub fn send_body(
    destination: &Destination,
    credential: &str,
    body: Vec<u8>,
    deadline: Instant,
    gate: &mut dyn SendGate,
    cancelled: &dyn Fn() -> bool,
) -> Result<PreparedRequest, SendError> {
    if gate.is_cancelled() || cancelled() {
        return Err(SendError::PolicyDenied);
    }
    let mut request = prepare(destination).map_err(classify)?;
    let mut body = Some(body);
    let mut sent = false;
    gate.write_authorized(&mut || {
        let body = body.take().ok_or(TransportError::InvalidRequest)?;
        request
            .send(credential, body, deadline, cancelled)
            .map_err(classify)?;
        sent = true;
        Ok(())
    })?;
    if !sent {
        return Err(SendError::InvalidResponse);
    }
    if gate.is_cancelled() || cancelled() {
        return Err(SendError::PolicyDenied);
    }
    Ok(request)
}

/// Receive after releasing the outbound send guard, so policy changes are not
/// blocked by a slow response. The caller must still validate its policy
/// revision and credential identity before accepting a conversion result.
pub fn receive_body(
    request: PreparedRequest,
    gate: &dyn SendGate,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<u8>, SendError> {
    let result = request.receive(&|| gate.is_cancelled() || cancelled());
    if gate.is_cancelled() || cancelled() {
        return Err(SendError::PolicyDenied);
    }
    result.map_err(|error| match error {
        Error::InvalidResponse => SendError::InvalidResponse,
        _ => SendError::Transport(classify(error)),
    })
}

fn classify(error: Error) -> TransportError {
    match error {
        Error::InvalidDestination
        | Error::InvalidPayload
        | Error::InvalidDeadline
        | Error::InvalidState => TransportError::InvalidRequest,
        Error::Timeout => TransportError::Timeout,
        Error::Cancelled => TransportError::Cancelled,
        Error::InvalidResponse => TransportError::InvalidResponse,
        _ => TransportError::Connection,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_preparation_refuses_diagnostic_http() {
        let destination =
            Destination::new(Scheme::HttpLoopback, "127.0.0.1", 8080, "/convert").unwrap();
        assert!(matches!(
            prepare(&destination),
            Err(Error::InvalidDestination)
        ));
    }

    #[test]
    fn receiving_requires_a_completed_send_and_denies_cancelled_results() {
        struct Gate(bool);
        impl SendGate for Gate {
            fn is_cancelled(&self) -> bool {
                self.0
            }
            fn write_authorized(
                &mut self,
                _: &mut dyn FnMut() -> Result<(), TransportError>,
            ) -> Result<(), SendError> {
                panic!("receive must not acquire the outbound send guard")
            }
        }
        let destination = Destination::new(Scheme::Https, "127.0.0.1", 1, "/convert").unwrap();
        assert_eq!(
            receive_body(prepare(&destination).unwrap(), &Gate(false), &|| false),
            Err(SendError::Transport(TransportError::InvalidRequest))
        );
        assert_eq!(
            receive_body(prepare(&destination).unwrap(), &Gate(true), &|| false),
            Err(SendError::PolicyDenied)
        );
    }

    #[test]
    fn denied_or_unused_send_gate_cannot_start_an_unguarded_request() {
        struct Gate(bool);
        impl SendGate for Gate {
            fn write_authorized(
                &mut self,
                _: &mut dyn FnMut() -> Result<(), TransportError>,
            ) -> Result<(), SendError> {
                if self.0 {
                    Ok(())
                } else {
                    Err(SendError::PolicyDenied)
                }
            }
        }
        let destination = Destination::new(Scheme::Https, "127.0.0.1", 1, "/convert").unwrap();
        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        assert!(matches!(
            send_body(
                &destination,
                "synthetic-key",
                vec![1],
                deadline,
                &mut Gate(false),
                &|| false
            ),
            Err(SendError::PolicyDenied)
        ));
        assert!(matches!(
            send_body(
                &destination,
                "synthetic-key",
                vec![1],
                deadline,
                &mut Gate(true),
                &|| false
            ),
            Err(SendError::InvalidResponse)
        ));
    }
}
