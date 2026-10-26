//! End-to-end check of the Host crate's policy runtime in a launched child.
use crate::bootstrap::{Launched, Mode};
use crate::job::{read_event, Inbox};
use crate::{Error, Result, Stop};
use std::path::Path;
use std::time::{Duration, Instant};
use sumibi_protocol::message::Id;
use sumibi_protocol::session::{ClientSession, Event};

pub fn diagnostic_policy_runtime_probe(path: &Path, default_host: bool) -> Result<()> {
    if default_host {
        return probe_once(path, true, false);
    }
    for reversed in [false, true] {
        probe_once(path, false, reversed)?;
    }
    Ok(())
}

fn probe_once(path: &Path, default_host: bool, reversed: bool) -> Result<()> {
    let stop = Stop::new()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let launched = Launched::connect_fanout(path, Mode::DiagnosticPolicyRuntime, &stop, deadline);
    if default_host {
        return if matches!(launched, Err(Error::Disconnected | Error::PolicyDenied)) {
            Ok(())
        } else {
            Err(Error::InvalidMessage)
        };
    }
    let (_child, mut first, mut second, boot, _) = launched?;
    if reversed {
        std::mem::swap(&mut first, &mut second);
    }
    let mut clients = [
        ClientSession::new(Id::new(1).ok_or(Error::InvalidMessage)?, boot.id()),
        ClientSession::new(Id::new(2).ok_or(Error::InvalidMessage)?, boot.id()),
    ];
    let mut inboxes = [Inbox::default(), Inbox::default()];
    for (index, client) in clients.iter_mut().enumerate() {
        let pipe = if index == 0 { &mut first } else { &mut second };
        pipe.write_all(
            client.hello().map_err(|_| Error::InvalidMessage)?,
            &stop,
            deadline,
        )?;
    }
    for (index, client) in clients.iter_mut().enumerate() {
        let pipe = if index == 0 { &mut first } else { &mut second };
        let Event::Ready(policy) = read_event(pipe, client, &mut inboxes[index], &stop, deadline)?
        else {
            return Err(Error::InvalidMessage);
        };
        if policy.settings != 1 || policy.authorization != 1 || policy.allowed {
            return Err(Error::InvalidMessage);
        }
    }
    for (authorization, allowed) in [(2, false), (3, true)] {
        for (index, client) in clients.iter_mut().enumerate() {
            let pipe = if index == 0 { &mut first } else { &mut second };
            let Event::PolicyChanged { policy, cancelled } =
                read_event(pipe, client, &mut inboxes[index], &stop, deadline)?
            else {
                return Err(Error::InvalidMessage);
            };
            if policy.settings != 1
                || policy.authorization != authorization
                || policy.allowed != allowed
                || cancelled.is_some()
            {
                return Err(Error::InvalidMessage);
            }
        }
    }
    let request = clients[0]
        .begin(1, "nihongo", 5000, Instant::now())
        .map_err(|_| Error::InvalidMessage)?;
    first.write_all(request, &stop, deadline)?;
    for (index, client) in clients.iter_mut().enumerate() {
        let pipe = if index == 0 { &mut first } else { &mut second };
        let Event::PolicyChanged { policy, cancelled } =
            read_event(pipe, client, &mut inboxes[index], &stop, deadline)?
        else {
            return Err(Error::InvalidMessage);
        };
        if policy.settings != 1
            || policy.authorization != 4
            || policy.allowed
            || cancelled != (index == 0).then_some(1)
        {
            return Err(Error::InvalidMessage);
        }
    }
    Ok(())
}
