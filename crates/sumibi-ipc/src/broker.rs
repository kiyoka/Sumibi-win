//! One authenticated request. The only implemented converter is an explicit
//! development fixture; production hosts advertise denied policy before text.
use crate::bootstrap::{Bootstrap, Mode};
use crate::{Error, Pipe, Result, Server, Stop};
use std::os::windows::io::AsRawHandle;
use std::time::{Duration, Instant};
use sumibi_protocol::message::{self, *};
use sumibi_protocol::Decoder;

pub fn serve_once(bootstrap: Bootstrap, diagnostic_enabled: bool) -> Result<()> {
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
        // This one-shot lockstep exchange has no pipelining or output queue.
        if decoded.consumed != read.bytes.len() {
            return Err(Error::InvalidMessage);
        }
        if let Some(body) = decoded.body {
            return message::decode(body).map_err(|_| Error::InvalidMessage);
        }
    }
}
