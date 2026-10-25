//! Authenticated one-shot or bounded sequential requests. The converter is an explicit
//! development fixture; production hosts advertise denied policy before text.
use crate::bootstrap::{Bootstrap, Mode, Service};
use crate::{Error, Pipe, Result, Server, Stop};
use std::os::windows::io::AsRawHandle;
use std::time::{Duration, Instant};
use sumibi_protocol::message::{self, *};
use sumibi_protocol::Decoder;

/// Dispatch only the explicit service selected by the restricted launcher.
pub fn serve(bootstrap: Bootstrap, diagnostic_enabled: bool) -> Result<()> {
    match bootstrap.service {
        Service::Once => serve_once(bootstrap, diagnostic_enabled),
        Service::Channel => serve_channel(bootstrap, diagnostic_enabled),
    }
}

pub fn serve_once(bootstrap: Bootstrap, diagnostic_enabled: bool) -> Result<()> {
    if bootstrap.service != Service::Once {
        return Err(Error::InvalidArgument);
    }
    let allowed = match bootstrap.mode {
        Mode::Disabled => false,
        Mode::DiagnosticFixed { .. } if diagnostic_enabled => true,
        _ => return Err(Error::PolicyDenied),
    };
    let server = Server::create(&bootstrap.name)?;
    bootstrap.ready()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let parent = crate::PinnedProcess::from_launch_handle(bootstrap.parent.as_handle())?;
    let mut pipe = server.accept(bootstrap.parent, &bootstrap.stop, deadline)?;
    let hello = read_message(&mut pipe, &bootstrap.stop, deadline)?;
    if !matches!(hello.message, Message::Hello(Hello { broker }) if broker == bootstrap.boot.id()) {
        return Err(Error::InvalidMessage);
    }
    let policy = Policy {
        settings: 1,
        authorization: 1,
        allowed,
    };
    let envelope = |message| Envelope {
        version: VERSION,
        session: hello.session,
        generation: hello.generation,
        message,
    };
    let bytes = message::encode(&envelope(Message::Ready(Ready {
        broker: bootstrap.boot.id(),
        policy,
    })))
    .map_err(|_| Error::InvalidMessage)?;
    pipe.write_all(bytes, &bootstrap.stop, deadline)?;
    if allowed {
        let request = read_message(&mut pipe, &bootstrap.stop, deadline)?;
        if request.session != hello.session || request.generation != hello.generation {
            return Err(Error::InvalidMessage);
        }
        let Message::Convert(convert) = request.message else {
            return Err(Error::InvalidMessage);
        };
        if convert.revision.settings != policy.settings
            || convert.revision.authorization != policy.authorization
        {
            return Err(Error::PolicyDenied);
        }
        let Mode::DiagnosticFixed { delay_ms } = bootstrap.mode else {
            return Err(Error::PolicyDenied);
        };
        // There is no backend or HTTP call in this fixture. Cancellation wakes
        // this wait immediately, even while the client has a pending read.
        if wait_stop(&bootstrap.stop, delay_ms)? {
            return Err(Error::Cancelled);
        }
        let reply = if delay_ms >= convert.timeout_ms {
            Message::Error(ConversionError {
                request: convert.request,
                revision: convert.revision,
                code: ErrorCode::Timeout,
            })
        } else {
            Message::Result(ConversionResult {
                request: convert.request,
                revision: convert.revision,
                text: "日本語".into(),
            })
        };
        drop(convert);
        pipe.write_all(
            message::encode(&envelope(reply)).map_err(|_| Error::InvalidMessage)?,
            &bootstrap.stop,
            deadline,
        )?;
    }
    // Keep the pinned process alive until the client has consumed the reply.
    // Parent teardown signals this event and reaps this specific child.
    // Parent death also wakes the idle host if the parent cannot run cleanup.
    unsafe {
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::Threading::WaitForMultipleObjects;
        let _ = WaitForMultipleObjects(
            &[
                HANDLE(bootstrap.stop.as_handle().as_raw_handle()),
                HANDLE(parent.as_handle().as_raw_handle()),
            ],
            false,
            10_000,
        );
    }
    Ok(())
}

fn serve_channel(bootstrap: Bootstrap, diagnostic_enabled: bool) -> Result<()> {
    let allowed = match bootstrap.mode {
        Mode::Disabled => false,
        Mode::DiagnosticFixed { .. } if diagnostic_enabled => true,
        Mode::DiagnosticRevoke if diagnostic_enabled => true,
        Mode::DiagnosticRevokeIdle if diagnostic_enabled => true,
        Mode::DiagnosticRevokeCoalesced if diagnostic_enabled => true,
        Mode::DiagnosticRevokeAfterResult if diagnostic_enabled => true,
        Mode::DiagnosticRegrantAfterResult if diagnostic_enabled => true,
        _ => return Err(Error::PolicyDenied),
    };
    let server = Server::create(&bootstrap.name)?;
    bootstrap.ready()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let parent = crate::PinnedProcess::from_launch_handle(bootstrap.parent.as_handle())?;
    let mut pipe = server.accept(bootstrap.parent, &bootstrap.stop, deadline)?;
    let hello = read_message(&mut pipe, &bootstrap.stop, deadline)?;
    if !matches!(hello.message, Message::Hello(Hello { broker }) if broker == bootstrap.boot.id()) {
        return Err(Error::InvalidMessage);
    }
    let policy = Policy {
        settings: 1,
        authorization: 1,
        allowed,
    };
    let envelope = |message| Envelope {
        version: VERSION,
        session: hello.session,
        generation: hello.generation,
        message,
    };
    let mut ready = message::encode(&envelope(Message::Ready(Ready {
        broker: bootstrap.boot.id(),
        policy,
    })))
    .map_err(|_| Error::InvalidMessage)?;
    if bootstrap.mode == Mode::DiagnosticRevokeCoalesced {
        ready.extend(
            message::encode(&envelope(Message::PolicyChanged(Policy {
                settings: policy.settings,
                authorization: policy.authorization + 1,
                allowed: false,
            })))
            .map_err(|_| Error::InvalidMessage)?,
        );
    }
    pipe.write_all(ready, &bootstrap.stop, deadline)?;
    if allowed {
        let lifetime = Instant::now() + crate::channel::MAX_LIFETIME;
        if bootstrap.mode == Mode::DiagnosticRevokeIdle {
            // Leave Ready as a separate read in this fixture, then publish a
            // policy change while the client has no outstanding request.
            if wait_stop(&bootstrap.stop, 150)? {
                return Err(Error::Cancelled);
            }
            pipe.write_all(
                message::encode(&envelope(Message::PolicyChanged(Policy {
                    settings: policy.settings,
                    authorization: policy.authorization + 1,
                    allowed: false,
                })))
                .map_err(|_| Error::InvalidMessage)?,
                &bootstrap.stop,
                (Instant::now() + Duration::from_secs(5)).min(lifetime),
            )?;
        }
        let mut last_request = 0;
        let max_requests = if matches!(
            bootstrap.mode,
            Mode::DiagnosticRevokeIdle | Mode::DiagnosticRevokeCoalesced
        ) {
            0
        } else {
            crate::channel::MAX_REQUESTS
        };
        for _ in 0..max_requests {
            // Lockstep: no pipelined frames, queued text, or unbounded resident
            // lifetime. Peer death and Stop also interrupt an idle read.
            let idle = (Instant::now() + crate::channel::IDLE_TIMEOUT).min(lifetime);
            let request = read_message(&mut pipe, &bootstrap.stop, idle)?;
            if request.session != hello.session || request.generation != hello.generation {
                return Err(Error::InvalidMessage);
            }
            let Message::Convert(convert) = request.message else {
                return Err(Error::InvalidMessage);
            };
            accept_request(&convert, policy, &mut last_request)?;
            let deadline =
                (Instant::now() + Duration::from_millis(convert.timeout_ms.into())).min(lifetime);
            if matches!(
                bootstrap.mode,
                Mode::DiagnosticRevokeAfterResult | Mode::DiagnosticRegrantAfterResult
            ) {
                let mut reply = message::encode(&envelope(Message::Result(ConversionResult {
                    request: convert.request,
                    revision: convert.revision,
                    text: "日本語".into(),
                })))
                .map_err(|_| Error::InvalidMessage)?;
                reply.extend(
                    message::encode(&envelope(Message::PolicyChanged(Policy {
                        settings: policy.settings,
                        // The second fixture coalesces revoke and re-consent:
                        // final allowed=true still invalidates the old result.
                        authorization: policy.authorization
                            + if bootstrap.mode == Mode::DiagnosticRegrantAfterResult {
                                2
                            } else {
                                1
                            },
                        allowed: bootstrap.mode == Mode::DiagnosticRegrantAfterResult,
                    })))
                    .map_err(|_| Error::InvalidMessage)?,
                );
                drop(convert);
                pipe.write_all(reply, &bootstrap.stop, deadline)?;
                break;
            }
            if bootstrap.mode == Mode::DiagnosticRevoke {
                // Exercise a policy change on the authenticated transport while
                // the client has an outstanding request. No result is sent.
                pipe.write_all(
                    message::encode(&envelope(Message::PolicyChanged(Policy {
                        settings: policy.settings,
                        authorization: policy.authorization + 1,
                        allowed: false,
                    })))
                    .map_err(|_| Error::InvalidMessage)?,
                    &bootstrap.stop,
                    deadline,
                )?;
                break;
            }
            let Mode::DiagnosticFixed { delay_ms } = bootstrap.mode else {
                return Err(Error::PolicyDenied);
            };
            if wait_stop(&bootstrap.stop, delay_ms.min(convert.timeout_ms))? {
                return Err(Error::Cancelled);
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout);
            }
            let reply = Message::Result(ConversionResult {
                request: convert.request,
                revision: convert.revision,
                text: "日本語".into(),
            });
            drop(convert);
            pipe.write_all(
                message::encode(&envelope(reply)).map_err(|_| Error::InvalidMessage)?,
                &bootstrap.stop,
                deadline,
            )?;
        }
    }
    // Do not exit between the final write and the client's authenticated read.
    // The launcher signals Stop when closing or refusing another request.
    unsafe {
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::Threading::WaitForMultipleObjects;
        let _ = WaitForMultipleObjects(
            &[
                HANDLE(bootstrap.stop.as_handle().as_raw_handle()),
                HANDLE(parent.as_handle().as_raw_handle()),
            ],
            false,
            10_000,
        );
    }
    Ok(())
}

fn accept_request(convert: &Convert, policy: Policy, last_request: &mut u64) -> Result<()> {
    if convert.request <= *last_request {
        return Err(Error::InvalidMessage);
    }
    if !policy.allowed
        || convert.revision.settings != policy.settings
        || convert.revision.authorization != policy.authorization
    {
        return Err(Error::PolicyDenied);
    }
    *last_request = convert.request;
    Ok(())
}

fn wait_stop(stop: &Stop, milliseconds: u32) -> Result<bool> {
    use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows::Win32::System::Threading::WaitForSingleObject;
    // SAFETY: borrowed live event, bounded worker-only wait.
    let status =
        unsafe { WaitForSingleObject(HANDLE(stop.as_handle().as_raw_handle()), milliseconds) };
    if status == WAIT_OBJECT_0 {
        Ok(true)
    } else if status == WAIT_TIMEOUT {
        Ok(false)
    } else {
        Err(Error::WorkerFailed)
    }
}

pub(crate) fn read_message(pipe: &mut Pipe, stop: &Stop, deadline: Instant) -> Result<Envelope> {
    let mut decoder = Decoder::default();
    loop {
        let read = pipe.start_read(stop, deadline)?.wait()?;
        let decoded = decoder
            .decode(&read.bytes)
            .map_err(|_| Error::InvalidMessage)?;
        // This lockstep exchange has no pipelining or output queue.
        if decoded.consumed != read.bytes.len() {
            return Err(Error::InvalidMessage);
        }
        if let Some(body) = decoded.body {
            return message::decode(body).map_err(|_| Error::InvalidMessage);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn policy() -> Policy {
        Policy {
            settings: 1,
            authorization: 1,
            allowed: true,
        }
    }
    fn request(id: u64) -> Convert {
        Convert {
            request: id,
            revision: Revision {
                input: 1,
                settings: 1,
                authorization: 1,
            },
            timeout_ms: 1000,
            text: "synthetic".into(),
        }
    }
    #[test]
    fn channel_rejects_replayed_and_wrapped_request_ids() {
        let mut last = 0;
        for id in [1, 8, u64::MAX] {
            assert_eq!(accept_request(&request(id), policy(), &mut last), Ok(()));
            assert_eq!(last, id);
        }
        for id in [0, 1, 8, u64::MAX] {
            assert_eq!(
                accept_request(&request(id), policy(), &mut last),
                Err(Error::InvalidMessage)
            );
            assert_eq!(last, u64::MAX);
        }
    }
    #[test]
    fn channel_policy_rejection_does_not_accept_the_request() {
        let mut last = 5;
        for change in 0..3 {
            let mut convert = request(6);
            let mut policy = policy();
            match change {
                0 => convert.revision.settings = 2,
                1 => convert.revision.authorization = 2,
                _ => policy.allowed = false,
            }
            assert_eq!(
                accept_request(&convert, policy, &mut last),
                Err(Error::PolicyDenied)
            );
            assert_eq!(last, 5);
        }
        assert_eq!(accept_request(&request(6), policy(), &mut last), Ok(()));
        assert_eq!(last, 6);
    }
}
