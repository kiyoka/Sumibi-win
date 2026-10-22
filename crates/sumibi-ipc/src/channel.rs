//! Reuse one explicitly launched, authenticated child for sequential requests.
//! Blocking worker-only API: launch/I/O/close/Drop must never run in a TSF or
//! window callback. No discovery, retry, COM target or UI dispatch is provided.
use crate::bootstrap::{Launched, Mode};
use crate::job::{read_event, Reservation};
use crate::{Error, Nonce, PinnedProcess, Pipe, Result, Stop};
use std::path::Path;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};
use sumibi_protocol::message::{self, Policy, Revision};
use sumibi_protocol::session::{ClientSession, Event, Outcome};

pub const MAX_REQUESTS: usize = 64;
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
pub const MAX_LIFETIME: Duration = Duration::from_secs(600);

struct Connected {
    // Drop the pipe before stopping/reaping the launched process.
    pipe: Pipe,
    client: ClientSession,
    policy: Policy,
    child: Launched,
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
    _delivery: Arc<Delivery>,
}
impl Reply {
    pub fn text(&self) -> &str {
        &self.text
    }
}

pub struct Channel {
    connected: Option<Connected>,
    stop: Stop,
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
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(Error::Timeout);
        }
        if remaining > Duration::from_secs(60) {
            return Err(Error::InvalidArgument);
        }
        let deadline = deadline.min(Instant::now() + Duration::from_secs(10));
        let expires = Instant::now() + MAX_LIFETIME;
        let (child, mut pipe, boot) = Launched::connect_channel(path, mode, stop, deadline)?;
        let mut client = ClientSession::new(Nonce::random()?.id(), boot.id());
        pipe.write_all(
            client.hello().map_err(|_| Error::InvalidMessage)?,
            stop,
            deadline,
        )?;
        let Event::Ready(policy) = read_event(&mut pipe, &mut client, stop, deadline)? else {
            return Err(Error::InvalidMessage);
        };
        if !policy.allowed {
            return Err(Error::PolicyDenied);
        }
        Ok(Self {
            connected: Some(Connected {
                pipe,
                client,
                policy,
                child,
            }),
            stop: stop.clone(),
            expires,
            completed: 0,
            delivery: Weak::new(),
            reservation,
        })
    }

    /// Query/synchronize-only duplicate of the exact launched process object.
    pub fn peer_process(&self) -> Result<PinnedProcess> {
        self.connected.as_ref().ok_or(Error::Closed)?.child.peer()
    }

    pub fn is_closed(&self) -> bool {
        self.connected.is_none()
    }

    /// Caller must already have reserved its input/target before calling this
    /// worker-only method. Nothing is queued or sent while a prior Reply lives.
    /// Every I/O/protocol/cancellation error retires this channel permanently.
    pub fn convert(&mut self, revision: Revision, text: &str, deadline: Instant) -> Result<Reply> {
        if self.connected.is_none() {
            return Err(Error::Closed);
        }
        if self.delivery.upgrade().is_some() {
            return Err(Error::Capacity);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if revision.input == 0
            || revision.settings == 0
            || revision.authorization == 0
            || !message::valid_text(text)
            || remaining > Duration::from_secs(60)
        {
            return Err(Error::InvalidArgument);
        }
        let result = self.exchange(revision, text, deadline.min(self.expires));
        match result {
            Ok((request, text)) => {
                self.completed += 1;
                let delivery = Arc::new(Delivery {
                    _reservation: self.reservation.clone(),
                });
                self.delivery = Arc::downgrade(&delivery);
                Ok(Reply {
                    request,
                    revision,
                    text,
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
        deadline: Instant,
    ) -> Result<(u64, String)> {
        if self.completed >= MAX_REQUESTS {
            return Err(Error::Retired);
        }
        if self.stop.is_cancelled()? {
            return Err(Error::Cancelled);
        }
        let milliseconds = deadline
            .saturating_duration_since(Instant::now())
            .as_millis();
        if milliseconds == 0 {
            return Err(Error::Timeout);
        }
        let connected = self.connected.as_mut().ok_or(Error::Closed)?;
        if connected.policy.settings != revision.settings
            || connected.policy.authorization != revision.authorization
        {
            return Err(Error::PolicyDenied);
        }
        let bytes = connected
            .client
            .begin(revision.input, text, milliseconds as u32, Instant::now())
            .map_err(|_| Error::InvalidMessage)?;
        let expected = connected
            .client
            .pending_request()
            .ok_or(Error::InvalidMessage)?;
        connected.pipe.write_all(bytes, &self.stop, deadline)?;
        match read_event(
            &mut connected.pipe,
            &mut connected.client,
            &self.stop,
            deadline,
        )? {
            Event::Reply {
                request,
                revision: actual,
                outcome: Outcome::Result(text),
            } if request == expected && actual == revision => Ok((request, text)),
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
