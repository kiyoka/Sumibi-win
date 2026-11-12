//! Reuse an authenticated launched child or independently pinned shared Host.
//! Blocking worker-only API: launch/I/O/close/Drop must never run in a TSF or
//! window callback. No discovery, retry, COM target or UI dispatch is provided.
use crate::bootstrap::{Launched, Mode};
use crate::conversion_log::timestamp_ms;
use crate::job::{read_event, reject_queued, Inbox, Reservation};
use crate::{Error, Nonce, PinnedProcess, Pipe, Result, Stop};
use std::path::Path;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};
use sumibi_core::candidate::CandidateItem;
use sumibi_protocol::message::{self, Policy, Revision};
use sumibi_protocol::session::{ClientSession, Event, Outcome};

pub const MAX_REQUESTS: usize = 64;
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
pub const MAX_LIFETIME: Duration = Duration::from_secs(600);

/// Timing sampled where the channel checks the effective deadline.
#[derive(Clone, Copy, Debug)]
pub struct ChannelTiming {
    pub opened_unix_ms: u64,
    pub ready_unix_ms: u64,
    pub expires_unix_ms: u64,
    pub checked_unix_ms: u64,
    pub connection_age_ms: u64,
    pub request_remaining_ms: u64,
    pub channel_remaining_ms: u64,
    pub channel_expired: bool,
}

struct Connected {
    // Drop the pipe before stopping/reaping the launched process.
    pipe: Pipe,
    client: ClientSession,
    inbox: Inbox,
    policy: Policy,
    peer: PinnedProcess,
    _child: Option<Launched>,
}
struct Delivery {
    _reservation: Arc<Reservation>,
}
/// Original result allocation and admission stay together until explicit drop.
/// No Debug/Clone/serialization; a retained reply prevents another request.
pub struct Reply {
    pub request: u64,
    pub revision: Revision,
    text: String,
    alternatives: Option<Vec<CandidateItem>>,
    deadline: Instant,
    _delivery: Arc<Delivery>,
}
impl Reply {
    pub fn alternatives(&self) -> Result<Option<&[CandidateItem]>> {
        self.text()?;
        Ok(self.alternatives.as_deref())
    }

    /// Successful text remains private after the originating request expires.
    pub fn text(&self) -> Result<&str> {
        if Instant::now() >= self.deadline {
            return Err(Error::Timeout);
        }
        Ok(&self.text)
    }
}

pub struct Channel {
    connected: Option<Connected>,
    stop: Stop,
    opened: Instant,
    opened_unix_ms: u64,
    ready_unix_ms: u64,
    expires: Instant,
    completed: usize,
    delivery: Weak<Delivery>,
    reservation: Arc<Reservation>,
}
impl Channel {
    /// Use a fresh Stop per channel. Shares the process's four admissions with
    /// one-shot jobs, including idle channels and replies retained after close.
    pub fn connect(path: &Path, mode: Mode, stop: &Stop, deadline: Instant) -> Result<Self> {
        Self::connect_reserved(
            path,
            mode,
            stop,
            deadline,
            Arc::new(Reservation::acquire()?),
        )
    }
    pub(crate) fn connect_reserved(
        path: &Path,
        mode: Mode,
        stop: &Stop,
        deadline: Instant,
        reservation: Arc<Reservation>,
    ) -> Result<Self> {
        let deadline = Self::connection_deadline(stop, deadline)?;
        let (child, pipe, boot) = Launched::connect_channel(path, mode, stop, deadline)?;
        let peer = child.peer()?;
        Self::establish(pipe, boot, peer, Some(child), stop, deadline, reservation)
    }

    /// Connect to an already-running Host using a process handle obtained
    /// through a trusted bootstrap. Pipe name and boot ID do not authenticate
    /// the Host. Closing this channel never stops the shared Host process.
    /// An unconsented Ready keeps the connection idle; `convert` still rejects
    /// it before framing source text. Policy changes retire the old channel.
    pub fn connect_pinned(
        name: &Nonce,
        boot: Nonce,
        peer: PinnedProcess,
        stop: &Stop,
        deadline: Instant,
    ) -> Result<Self> {
        Self::connect_pinned_reserved(
            name,
            boot,
            peer,
            stop,
            deadline,
            Arc::new(Reservation::acquire()?),
        )
    }

    pub(crate) fn connect_pinned_reserved(
        name: &Nonce,
        boot: Nonce,
        peer: PinnedProcess,
        stop: &Stop,
        deadline: Instant,
        reservation: Arc<Reservation>,
    ) -> Result<Self> {
        let deadline = Self::connection_deadline(stop, deadline)?;
        let retained_peer = PinnedProcess::from_launch_handle(peer.as_handle())?;
        let pipe = Pipe::connect(name, peer)?;
        Self::establish(pipe, boot, retained_peer, None, stop, deadline, reservation)
    }

    fn connection_deadline(stop: &Stop, deadline: Instant) -> Result<Instant> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(Error::Timeout);
        }
        if remaining > Duration::from_secs(60) {
            return Err(Error::InvalidArgument);
        }
        if stop.is_cancelled()? {
            return Err(Error::Cancelled);
        }
        Ok(deadline.min(Instant::now() + Duration::from_secs(10)))
    }

    fn establish(
        mut pipe: Pipe,
        boot: Nonce,
        peer: PinnedProcess,
        child: Option<Launched>,
        stop: &Stop,
        deadline: Instant,
        reservation: Arc<Reservation>,
    ) -> Result<Self> {
        let opened = Instant::now();
        let opened_unix_ms = timestamp_ms();
        let expires = opened + MAX_LIFETIME;
        let mut client = ClientSession::new(Nonce::random()?.id(), boot.id());
        let mut inbox = Inbox::default();
        pipe.write_all(
            client.hello().map_err(|_| Error::InvalidMessage)?,
            stop,
            deadline,
        )?;
        let Event::Ready(policy) = read_event(&mut pipe, &mut client, &mut inbox, stop, deadline)?
        else {
            crate::debug_log_ipc("establish: expected Event::Ready, but got other message");
            return Err(Error::InvalidMessage);
        };
        let ready_unix_ms = timestamp_ms();
        crate::debug_log_ipc(&format!(
            "establish: Event::Ready received, policy.allowed={}",
            policy.allowed
        ));
        // A shared Host starts without consent. Keep its authenticated channel
        // idle until a policy notice retires it; reconnect once on that notice.
        // A launched one-shot diagnostic retains its denial semantics.
        if !policy.allowed && child.is_some() {
            crate::debug_log_ipc("establish: policy.allowed is FALSE, returning PolicyDenied");
            return Err(Error::PolicyDenied);
        }
        Ok(Self {
            connected: Some(Connected {
                pipe,
                client,
                inbox,
                policy,
                peer,
                _child: child,
            }),
            stop: stop.clone(),
            opened,
            opened_unix_ms,
            ready_unix_ms,
            expires,
            completed: 0,
            delivery: Weak::new(),
            reservation,
        })
    }

    /// Query/synchronize-only duplicate of the exact pinned process object.
    pub fn peer_process(&self) -> Result<PinnedProcess> {
        PinnedProcess::from_launch_handle(
            self.connected
                .as_ref()
                .ok_or(Error::Closed)?
                .peer
                .as_handle(),
        )
    }

    /// The policy received at Ready. A later queued notice is checked before
    /// sending source text; callers must not treat this snapshot as a grant.
    pub fn policy(&self) -> Option<Policy> {
        self.connected.as_ref().map(|connected| connected.policy)
    }

    /// A prepared worker calls this only on its I/O thread. A queued policy
    /// change retires the channel; callers still revalidate before capture.
    pub(crate) fn check_idle_policy(&mut self, deadline: Instant) -> Result<()> {
        let connected = self.connected.as_mut().ok_or(Error::Closed)?;
        let result = reject_queued(
            &mut connected.pipe,
            &mut connected.client,
            &mut connected.inbox,
            &self.stop,
            deadline,
        );
        if result.is_err() {
            self.close();
        }
        result
    }

    pub fn is_closed(&self) -> bool {
        self.connected.is_none()
    }

    /// Caller must already have reserved its input/target before calling this
    /// worker-only method. Nothing is queued or sent while a prior Reply lives.
    /// Every I/O/protocol/cancellation error retires this channel permanently.
    pub fn convert(&mut self, revision: Revision, text: &str, deadline: Instant) -> Result<Reply> {
        self.convert_with_current(revision, text, None, deadline)
    }

    pub fn convert_with_current(
        &mut self,
        revision: Revision,
        text: &str,
        current_result: Option<&str>,
        deadline: Instant,
    ) -> Result<Reply> {
        self.convert_timed(revision, text, current_result, deadline, &mut None)
    }

    pub(crate) fn convert_timed(
        &mut self,
        revision: Revision,
        text: &str,
        current_result: Option<&str>,
        request_deadline: Instant,
        timing: &mut Option<ChannelTiming>,
    ) -> Result<Reply> {
        if self.connected.is_none() {
            return Err(Error::Closed);
        }
        if self.delivery.upgrade().is_some() {
            return Err(Error::Capacity);
        }
        let remaining = request_deadline.saturating_duration_since(Instant::now());
        if revision.input == 0
            || revision.settings == 0
            || revision.authorization == 0
            || !message::valid_text(text)
            || remaining > Duration::from_secs(60)
        {
            return Err(Error::InvalidArgument);
        }
        let deadline = request_deadline.min(self.expires);
        let result = self.exchange(
            revision,
            text,
            current_result,
            deadline,
            request_deadline,
            timing,
        );
        match result {
            Ok((request, text, alternatives)) => {
                self.completed += 1;
                let delivery = Arc::new(Delivery {
                    _reservation: self.reservation.clone(),
                });
                self.delivery = Arc::downgrade(&delivery);
                Ok(Reply {
                    request,
                    revision,
                    text,
                    alternatives,
                    deadline,
                    _delivery: delivery,
                })
            }
            Err(error) => {
                self.close();
                Err(error)
            }
        }
    }

    fn exchange(
        &mut self,
        revision: Revision,
        text: &str,
        current_result: Option<&str>,
        deadline: Instant,
        request_deadline: Instant,
        timing: &mut Option<ChannelTiming>,
    ) -> Result<(u64, String, Option<Vec<CandidateItem>>)> {
        if self.completed >= MAX_REQUESTS {
            return Err(Error::Retired);
        }
        if self.stop.is_cancelled()? {
            return Err(Error::Cancelled);
        }
        let now = Instant::now();
        *timing = Some(ChannelTiming {
            opened_unix_ms: self.opened_unix_ms,
            ready_unix_ms: self.ready_unix_ms,
            expires_unix_ms: self
                .opened_unix_ms
                .saturating_add(MAX_LIFETIME.as_millis() as u64),
            checked_unix_ms: timestamp_ms(),
            connection_age_ms: now.saturating_duration_since(self.opened).as_millis() as u64,
            request_remaining_ms: request_deadline.saturating_duration_since(now).as_millis()
                as u64,
            channel_remaining_ms: self.expires.saturating_duration_since(now).as_millis() as u64,
            channel_expired: now >= self.expires,
        });
        let milliseconds = deadline.saturating_duration_since(now).as_millis();
        if milliseconds == 0 {
            return Err(Error::Timeout);
        }
        let connected = self.connected.as_mut().ok_or(Error::Closed)?;
        if !connected.policy.allowed
            || connected.policy.settings != revision.settings
            || connected.policy.authorization != revision.authorization
        {
            return Err(Error::PolicyDenied);
        }
        // An idle policy notice already queued by the authenticated broker
        // must retire this request before its text is copied into a frame.
        reject_queued(
            &mut connected.pipe,
            &mut connected.client,
            &mut connected.inbox,
            &self.stop,
            deadline,
        )?;
        let bytes = connected
            .client
            .begin_conversion(
                revision.input,
                text,
                current_result,
                milliseconds as u32,
                Instant::now(),
            )
            .map_err(|_| Error::InvalidMessage)?;
        let expected = connected
            .client
            .pending_request()
            .ok_or(Error::InvalidMessage)?;
        connected.pipe.write_all(bytes, &self.stop, deadline)?;
        match read_event(
            &mut connected.pipe,
            &mut connected.client,
            &mut connected.inbox,
            &self.stop,
            deadline,
        )? {
            Event::Reply {
                request,
                revision: actual,
                outcome,
            } if request == expected && actual == revision => {
                let (text, alternatives) = match (outcome, current_result) {
                    (Outcome::Result(text), None) => (text, None),
                    (Outcome::Alternatives(items), Some(current))
                        if message::valid_alternatives(current, &items) =>
                    {
                        (current.to_owned(), Some(items))
                    }
                    _ => return Err(Error::InvalidMessage),
                };
                reject_queued(
                    &mut connected.pipe,
                    &mut connected.client,
                    &mut connected.inbox,
                    &self.stop,
                    deadline,
                )?;
                Ok((request, text, alternatives))
            }
            Event::PolicyChanged {
                cancelled: Some(request),
                ..
            } if request == expected => Err(Error::PolicyDenied),
            Event::Expired(_) => Err(Error::Timeout),
            _ => Err(Error::InvalidMessage),
        }
    }

    /// Blocking bounded child cleanup; call on the worker, outside callbacks.
    /// The facade and replies keep their admission until both are dropped.
    pub fn close(&mut self) {
        if let Some(mut connected) = self.connected.take() {
            connected.client.disconnect();
            drop(connected);
        }
    }
}
