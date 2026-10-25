//! Authenticated one-shot or bounded sequential requests. The converter is an explicit
//! development fixture; production hosts advertise denied policy before text.
use crate::bootstrap::{Bootstrap, Mode, Service};
use crate::{Error, Pipe, Result, Server, Stop};
use std::os::windows::io::AsRawHandle;
use std::sync::{RwLock, RwLockReadGuard};
use std::time::{Duration, Instant};
use sumibi_protocol::broker_session::{BrokerError, BrokerSession, Event as BrokerEvent};
use sumibi_protocol::message::{self, *};
use sumibi_protocol::policy_fanout::{FanoutError, PolicyFanout};
use sumibi_protocol::Decoder;

const DIAGNOSTIC_CONNECTION: u64 = 1;

/// Broker-local policy authority. A future settings owner may update this
/// value while the worker is waiting; every accepted request reads it afresh.
/// The diagnostic host currently supplies only synthetic changes.
pub struct BrokerPolicyGate {
    state: RwLock<Policy>,
}

/// Keeps an authorized result publication ahead of the next policy update.
/// Dropping it after the bounded pipe write is the publication point.
struct PublishGuard<'a> {
    _state: RwLockReadGuard<'a, Policy>,
}
impl BrokerPolicyGate {
    pub fn new(initial: Policy) -> Result<Self> {
        if !initial.valid() {
            return Err(Error::InvalidArgument);
        }
        Ok(Self {
            state: RwLock::new(initial),
        })
    }
    pub fn snapshot(&self) -> Result<Policy> {
        self.state
            .read()
            .map(|state| *state)
            .map_err(|_| Error::PolicyDenied)
    }
    // Synthetic coalescing fixture only. Product updates must use publish so
    // admission and connection notification cannot diverge after an error.
    fn update(&self, next: Policy) -> Result<()> {
        let mut state = self.state.write().map_err(|_| Error::PolicyDenied)?;
        if !next.follows(*state) {
            return Err(Error::PolicyDenied);
        }
        *state = next;
        Ok(())
    }
    /// Move the admission gate and every Broker session to one epoch while
    /// result publication is excluded. The caller closes retired transports
    /// and completes each notice write before accepting another request.
    pub fn publish(&self, order: &mut PolicyFanout, next: Policy) -> Result<Vec<u64>> {
        let mut state = self.state.write().map_err(|_| Error::PolicyDenied)?;
        if !next.follows(*state) {
            return Err(Error::PolicyDenied);
        }
        let retired = order.publish(next).map_err(map_fanout_error)?;
        *state = next;
        Ok(retired)
    }
    pub fn authorize(&self, revision: Revision) -> Result<()> {
        let state = self.snapshot()?;
        if !state.allowed
            || revision.settings != state.settings
            || revision.authorization != state.authorization
        {
            return Err(Error::PolicyDenied);
        }
        Ok(())
    }

    fn authorize_for_publish(&self, revision: Revision) -> Result<PublishGuard<'_>> {
        let state = self.state.read().map_err(|_| Error::PolicyDenied)?;
        if !state.allowed
            || revision.settings != state.settings
            || revision.authorization != state.authorization
        {
            return Err(Error::PolicyDenied);
        }
        Ok(PublishGuard { _state: state })
    }
}

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
    let policy = Policy {
        settings: 1,
        authorization: 1,
        allowed,
    };
    let mut order = BrokerSession::new(bootstrap.boot.id(), policy).map_err(map_broker_error)?;
    let BrokerEvent::Ready(ready) = order
        .receive(hello, Instant::now())
        .map_err(map_broker_error)?
    else {
        return Err(Error::InvalidMessage);
    };
    let bytes = message::encode(&ready).map_err(|_| Error::InvalidMessage)?;
    pipe.write_all(bytes, &bootstrap.stop, deadline)?;
    if allowed {
        let request = read_message(&mut pipe, &bootstrap.stop, deadline)?;
        let BrokerEvent::Convert(convert) = order
            .receive(request, Instant::now())
            .map_err(map_broker_error)?
        else {
            return Err(Error::InvalidMessage);
        };
        let Mode::DiagnosticFixed { delay_ms } = bootstrap.mode else {
            return Err(Error::PolicyDenied);
        };
        // There is no backend or HTTP call in this fixture. Cancellation wakes
        // this wait immediately, even while the client has a pending read.
        if wait_stop(&bootstrap.stop, delay_ms)? {
            return Err(Error::Cancelled);
        }
        let reply = if delay_ms >= convert.timeout_ms {
            order.error(
                convert.request,
                convert.revision,
                ErrorCode::Timeout,
                Instant::now(),
            )
        } else {
            order.result(
                convert.request,
                convert.revision,
                "日本語".into(),
                Instant::now(),
            )
        }
        .map_err(map_broker_error)?
        .ok_or(Error::InvalidMessage)?;
        drop(convert);
        pipe.write_all(
            message::encode(&reply).map_err(|_| Error::InvalidMessage)?,
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
        Mode::DiagnosticSettingsAfterResult if diagnostic_enabled => true,
        Mode::DiagnosticRegrantAfterResult if diagnostic_enabled => true,
        Mode::DiagnosticRevokeAtAccept if diagnostic_enabled => true,
        Mode::DiagnosticRegrantAtAccept if diagnostic_enabled => true,
        Mode::DiagnosticFutureReply if diagnostic_enabled => true,
        _ => return Err(Error::PolicyDenied),
    };
    let server = Server::create(&bootstrap.name)?;
    bootstrap.ready()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let parent = crate::PinnedProcess::from_launch_handle(bootstrap.parent.as_handle())?;
    let mut pipe = server.accept(bootstrap.parent, &bootstrap.stop, deadline)?;
    let hello = read_message(&mut pipe, &bootstrap.stop, deadline)?;
    let policy = Policy {
        settings: 1,
        authorization: 1,
        allowed,
    };
    let gate = BrokerPolicyGate::new(policy)?;
    let mut order = PolicyFanout::new(bootstrap.boot.id(), policy).map_err(map_fanout_error)?;
    let session = hello.session;
    let generation = hello.generation;
    let envelope = |message| Envelope {
        version: VERSION,
        session,
        generation,
        message,
    };
    let ready_frame = order
        .attach(DIAGNOSTIC_CONNECTION, hello, Instant::now())
        .map_err(map_fanout_error)?;
    let mut ready = message::encode(&ready_frame).map_err(|_| Error::InvalidMessage)?;
    let mut coalesced_policy = None;
    if bootstrap.mode == Mode::DiagnosticRevokeCoalesced {
        let next = Policy {
            settings: policy.settings,
            authorization: policy.authorization + 1,
            allowed: false,
        };
        if !gate.publish(&mut order, next)?.is_empty() {
            return Err(Error::InvalidMessage);
        }
        let (notice, _) = pending_notice(&order)?;
        ready.extend(notice);
        coalesced_policy = Some(next);
    }
    pipe.write_all(ready, &bootstrap.stop, deadline)?;
    order
        .ready_delivered(DIAGNOSTIC_CONNECTION)
        .map_err(map_fanout_error)?;
    if let Some(delivered) = coalesced_policy {
        order
            .notice_delivered(DIAGNOSTIC_CONNECTION, delivered)
            .map_err(map_fanout_error)?;
    }
    if allowed {
        let lifetime = Instant::now() + crate::channel::MAX_LIFETIME;
        if bootstrap.mode == Mode::DiagnosticRevokeIdle {
            // Leave Ready as a separate read in this fixture, then publish a
            // policy change while the client has no outstanding request.
            if wait_stop(&bootstrap.stop, 150)? {
                return Err(Error::Cancelled);
            }
            let next = Policy {
                settings: policy.settings,
                authorization: policy.authorization + 1,
                allowed: false,
            };
            if !gate.publish(&mut order, next)?.is_empty() {
                return Err(Error::InvalidMessage);
            }
            let (notice, _) = pending_notice(&order)?;
            pipe.write_all(
                notice,
                &bootstrap.stop,
                (Instant::now() + Duration::from_secs(5)).min(lifetime),
            )?;
            order
                .notice_delivered(DIAGNOSTIC_CONNECTION, next)
                .map_err(map_fanout_error)?;
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
            let BrokerEvent::Convert(convert) = order
                .receive(DIAGNOSTIC_CONNECTION, request, Instant::now())
                .map_err(map_fanout_error)?
            else {
                return Err(Error::InvalidMessage);
            };
            if matches!(
                bootstrap.mode,
                Mode::DiagnosticRevokeAtAccept | Mode::DiagnosticRegrantAtAccept
            ) {
                let revoked = Policy {
                    settings: policy.settings,
                    authorization: policy.authorization + 1,
                    allowed: false,
                };
                gate.update(revoked)?;
                let latest = if bootstrap.mode == Mode::DiagnosticRegrantAtAccept {
                    let regranted = Policy {
                        settings: policy.settings,
                        authorization: policy.authorization + 2,
                        allowed: true,
                    };
                    gate.update(regranted)?;
                    regranted
                } else {
                    revoked
                };
                if accept_request(&convert, &gate, &mut last_request) != Err(Error::PolicyDenied) {
                    return Err(Error::InvalidMessage);
                }
                if last_request != 0 {
                    return Err(Error::InvalidMessage);
                }
                if !gate.publish(&mut order, latest)?.is_empty() {
                    return Err(Error::InvalidMessage);
                }
                let (notice, cancelled) = pending_notice(&order)?;
                if cancelled != Some(convert.request) {
                    return Err(Error::InvalidMessage);
                }
                drop(convert);
                pipe.write_all(notice, &bootstrap.stop, idle)?;
                order
                    .notice_delivered(DIAGNOSTIC_CONNECTION, latest)
                    .map_err(map_fanout_error)?;
                break;
            }
            accept_request(&convert, &gate, &mut last_request)?;
            let deadline =
                (Instant::now() + Duration::from_millis(convert.timeout_ms.into())).min(lifetime);
            if bootstrap.mode == Mode::DiagnosticFutureReply {
                let future = convert
                    .request
                    .checked_add(1)
                    .ok_or(Error::InvalidMessage)?;
                let reply = Message::Result(ConversionResult {
                    request: future,
                    revision: convert.revision,
                    text: "日本語".into(),
                });
                drop(convert);
                pipe.write_all(
                    message::encode(&envelope(reply)).map_err(|_| Error::InvalidMessage)?,
                    &bootstrap.stop,
                    deadline,
                )?;
                break;
            }
            if matches!(
                bootstrap.mode,
                Mode::DiagnosticRevokeAfterResult
                    | Mode::DiagnosticSettingsAfterResult
                    | Mode::DiagnosticRegrantAfterResult
            ) {
                let result = order
                    .result(
                        DIAGNOSTIC_CONNECTION,
                        convert.request,
                        convert.revision,
                        "日本語".into(),
                        Instant::now(),
                    )
                    .map_err(map_fanout_error)?
                    .ok_or(Error::InvalidMessage)?;
                let mut reply = message::encode(&result).map_err(|_| Error::InvalidMessage)?;
                let next = if bootstrap.mode == Mode::DiagnosticSettingsAfterResult {
                    Policy {
                        settings: policy.settings + 1,
                        ..policy
                    }
                } else {
                    Policy {
                        settings: policy.settings,
                        // Re-consent has allowed=true but still invalidates the old result.
                        authorization: policy.authorization
                            + if bootstrap.mode == Mode::DiagnosticRegrantAfterResult {
                                2
                            } else {
                                1
                            },
                        allowed: bootstrap.mode == Mode::DiagnosticRegrantAfterResult,
                    }
                };
                if !gate.publish(&mut order, next)?.is_empty() {
                    return Err(Error::InvalidMessage);
                }
                let (notice, cancelled) = pending_notice(&order)?;
                if cancelled.is_some() {
                    return Err(Error::InvalidMessage);
                }
                reply.extend(notice);
                drop(convert);
                pipe.write_all(reply, &bootstrap.stop, deadline)?;
                order
                    .notice_delivered(DIAGNOSTIC_CONNECTION, next)
                    .map_err(map_fanout_error)?;
                break;
            }
            if bootstrap.mode == Mode::DiagnosticRevoke {
                // Exercise a policy change on the authenticated transport while
                // the client has an outstanding request. No result is sent.
                let revoked = Policy {
                    settings: policy.settings,
                    authorization: policy.authorization + 1,
                    allowed: false,
                };
                if !gate.publish(&mut order, revoked)?.is_empty() {
                    return Err(Error::InvalidMessage);
                }
                if gate.authorize(convert.revision) != Err(Error::PolicyDenied) {
                    return Err(Error::InvalidMessage);
                }
                let (notice, cancelled) = pending_notice(&order)?;
                if cancelled != Some(convert.request) {
                    return Err(Error::InvalidMessage);
                }
                pipe.write_all(notice, &bootstrap.stop, deadline)?;
                order
                    .notice_delivered(DIAGNOSTIC_CONNECTION, revoked)
                    .map_err(map_fanout_error)?;
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
            let _publication = gate.authorize_for_publish(convert.revision)?;
            let reply = order
                .result(
                    DIAGNOSTIC_CONNECTION,
                    convert.request,
                    convert.revision,
                    "日本語".into(),
                    Instant::now(),
                )
                .map_err(map_fanout_error)?
                .ok_or(Error::InvalidMessage)?;
            drop(convert);
            pipe.write_all(
                message::encode(&reply).map_err(|_| Error::InvalidMessage)?,
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

fn map_broker_error(error: BrokerError) -> Error {
    match error {
        BrokerError::PolicyDenied => Error::PolicyDenied,
        BrokerError::Expired => Error::Timeout,
        _ => Error::InvalidMessage,
    }
}

fn map_fanout_error(error: FanoutError) -> Error {
    match error {
        FanoutError::Broker(error) => map_broker_error(error),
        FanoutError::InvalidPolicy => Error::PolicyDenied,
        _ => Error::InvalidMessage,
    }
}

fn pending_notice(order: &PolicyFanout) -> Result<(Vec<u8>, Option<u64>)> {
    let notice = order
        .pending_notice(DIAGNOSTIC_CONNECTION)
        .map_err(map_fanout_error)?
        .ok_or(Error::InvalidMessage)?;
    Ok((
        message::encode(&notice.frame).map_err(|_| Error::InvalidMessage)?,
        notice.cancelled,
    ))
}

fn accept_request(
    convert: &Convert,
    gate: &BrokerPolicyGate,
    last_request: &mut u64,
) -> Result<()> {
    if convert.request <= *last_request {
        return Err(Error::InvalidMessage);
    }
    gate.authorize(convert.revision)?;
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
        let gate = BrokerPolicyGate::new(policy()).unwrap();
        for id in [1, 8, u64::MAX] {
            assert_eq!(accept_request(&request(id), &gate, &mut last), Ok(()));
            assert_eq!(last, id);
        }
        for id in [0, 1, 8, u64::MAX] {
            assert_eq!(
                accept_request(&request(id), &gate, &mut last),
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
            let gate = BrokerPolicyGate::new(policy).unwrap();
            assert_eq!(
                accept_request(&convert, &gate, &mut last),
                Err(Error::PolicyDenied)
            );
            assert_eq!(last, 5);
        }
        let gate = BrokerPolicyGate::new(policy()).unwrap();
        assert_eq!(accept_request(&request(6), &gate, &mut last), Ok(()));
        assert_eq!(last, 6);
    }

    #[test]
    fn broker_policy_updates_reject_stale_requests_before_acceptance() {
        let gate = BrokerPolicyGate::new(policy()).unwrap();
        let mut last = 0;
        assert_eq!(accept_request(&request(1), &gate, &mut last), Ok(()));
        let revoked = Policy {
            settings: 1,
            authorization: 2,
            allowed: false,
        };
        assert_eq!(gate.update(revoked), Ok(()));
        assert_eq!(
            accept_request(&request(2), &gate, &mut last),
            Err(Error::PolicyDenied)
        );
        assert_eq!(last, 1);
        assert_eq!(gate.update(policy()), Err(Error::PolicyDenied));
        assert_eq!(
            gate.update(Policy {
                settings: 1,
                authorization: 2,
                allowed: true,
            }),
            Err(Error::PolicyDenied)
        );
        let regranted = Policy {
            settings: 1,
            authorization: 3,
            allowed: true,
        };
        assert_eq!(gate.update(regranted), Ok(()));
        assert_eq!(
            accept_request(&request(2), &gate, &mut last),
            Err(Error::PolicyDenied)
        );
        let mut current = request(2);
        current.revision.authorization = 3;
        assert_eq!(accept_request(&current, &gate, &mut last), Ok(()));
        assert_eq!(last, 2);
    }

    #[test]
    fn settings_revision_can_advance_without_changing_authorization() {
        let gate = BrokerPolicyGate::new(policy()).unwrap();
        let updated = Policy {
            settings: 2,
            authorization: 1,
            allowed: true,
        };
        assert_eq!(gate.update(updated), Ok(()));
        let mut last = 0;
        assert_eq!(
            accept_request(&request(1), &gate, &mut last),
            Err(Error::PolicyDenied)
        );
        assert_eq!(last, 0);
        let mut current = request(1);
        current.revision.settings = 2;
        assert_eq!(accept_request(&current, &gate, &mut last), Ok(()));
        assert_eq!(last, 1);
        assert_eq!(
            gate.update(Policy {
                allowed: false,
                ..updated
            }),
            Err(Error::PolicyDenied)
        );
    }

    #[test]
    fn result_publication_guard_serializes_policy_update() {
        let gate = BrokerPolicyGate::new(policy()).unwrap();
        let publication = gate.authorize_for_publish(request(1).revision).unwrap();
        assert!(gate.state.try_write().is_err());
        drop(publication);
        assert!(gate.state.try_write().is_ok());

        gate.update(Policy {
            settings: 1,
            authorization: 2,
            allowed: false,
        })
        .unwrap();
        assert!(matches!(
            gate.authorize_for_publish(request(1).revision),
            Err(Error::PolicyDenied)
        ));
    }

    #[test]
    fn failed_fanout_does_not_advance_admission_policy() {
        let broker = Id::new(90).unwrap();
        let gate = BrokerPolicyGate::new(policy()).unwrap();
        let advanced = Policy {
            settings: 2,
            ..policy()
        };
        let mut ahead = PolicyFanout::new(broker, advanced).unwrap();
        assert!(matches!(
            gate.publish(&mut ahead, policy()),
            Err(Error::PolicyDenied)
        ));
        assert!(gate.snapshot().unwrap() == policy());
        assert!(ahead.policy() == advanced);

        let mut fanout = PolicyFanout::new(broker, policy()).unwrap();
        let session = Id::new(1).unwrap();
        fanout
            .attach(
                1,
                Envelope {
                    version: VERSION,
                    session,
                    generation: 1,
                    message: Message::Hello(Hello { broker }),
                },
                Instant::now(),
            )
            .unwrap();
        fanout.ready_delivered(1).unwrap();
        assert!(gate.publish(&mut fanout, advanced).unwrap().is_empty());
        assert!(gate.snapshot().unwrap() == advanced);
        assert!(fanout.policy() == advanced);
        assert!(fanout.pending_notice(1).unwrap().is_some());
        assert_eq!(
            gate.authorize(request(1).revision),
            Err(Error::PolicyDenied)
        );
    }
}
