//! End-to-end check of the Host crate's policy runtime in a launched child.
use crate::bootstrap::{Launched, Mode};
use crate::job::{read_event, Inbox};
use crate::{Error, Pipe, Result, Stop};
use std::path::Path;
use std::time::{Duration, Instant};
use sumibi_protocol::message::Id;
use sumibi_protocol::session::{ClientSession, Event, Outcome};

pub fn diagnostic_policy_runtime_probe(path: &Path, default_host: bool) -> Result<()> {
    if default_host {
        return probe_once(path, true, false, false);
    }
    for reversed in [false, true] {
        probe_once(path, false, reversed, false)?;
    }
    probe_once(path, false, false, true)?;
    Ok(())
}

fn probe_once(path: &Path, default_host: bool, reversed: bool, staggered: bool) -> Result<()> {
    let stop = Stop::new()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let launched = if staggered {
        Launched::connect_policy_runtime_first(path, &stop, deadline)
            .map(|(child, first, name, boot)| (child, first, None, boot, name))
    } else {
        Launched::connect_fanout(path, Mode::DiagnosticPolicyRuntime, &stop, deadline)
            .map(|(child, first, second, boot, name)| (child, first, Some(second), boot, name))
    };
    if default_host {
        return if matches!(launched, Err(Error::Disconnected | Error::PolicyDenied)) {
            Ok(())
        } else {
            Err(Error::InvalidMessage)
        };
    }
    let (child, mut first, second, boot, name) = launched?;
    let mut clients = [
        ClientSession::new(Id::new(1).ok_or(Error::InvalidMessage)?, boot.id()),
        ClientSession::new(Id::new(2).ok_or(Error::InvalidMessage)?, boot.id()),
    ];
    let mut inboxes = [Inbox::default(), Inbox::default()];
    let mut second = if let Some(mut second) = second {
        if reversed {
            std::mem::swap(&mut first, &mut second);
        }
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
            verify_ready(pipe, client, &mut inboxes[index], &stop, deadline)?;
        }
        second
    } else {
        // The first Ready must arrive while the second OS accept is pending.
        first.write_all(
            clients[0].hello().map_err(|_| Error::InvalidMessage)?,
            &stop,
            deadline,
        )?;
        verify_ready(
            &mut first,
            &mut clients[0],
            &mut inboxes[0],
            &stop,
            deadline,
        )?;
        let mut second = Pipe::connect(&name, child.peer()?)?;
        second.write_all(
            clients[1].hello().map_err(|_| Error::InvalidMessage)?,
            &stop,
            deadline,
        )?;
        verify_ready(
            &mut second,
            &mut clients[1],
            &mut inboxes[1],
            &stop,
            deadline,
        )?;
        second
    };
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
    // The second connection may become readable first; the Host must retain
    // its frame while waiting for the other authenticated request.
    let second_request = clients[1]
        .begin(1, "second", 5000, Instant::now())
        .map_err(|_| Error::InvalidMessage)?;
    second.write_all(second_request, &stop, deadline)?;
    let request = clients[0]
        .begin(1, "nihongo", 5000, Instant::now())
        .map_err(|_| Error::InvalidMessage)?;
    first.write_all(request, &stop, deadline)?;
    let Event::Reply {
        request: 1,
        outcome: Outcome::Result(text),
        ..
    } = read_event(
        &mut second,
        &mut clients[1],
        &mut inboxes[1],
        &stop,
        deadline,
    )?
    else {
        return Err(Error::InvalidMessage);
    };
    if text != "完了" {
        return Err(Error::InvalidMessage);
    }
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
    for (index, client) in clients.iter_mut().enumerate() {
        let pipe = if index == 0 { &mut first } else { &mut second };
        let Event::PolicyChanged { policy, cancelled } =
            read_event(pipe, client, &mut inboxes[index], &stop, deadline)?
        else {
            return Err(Error::InvalidMessage);
        };
        if policy.settings != 1
            || policy.authorization != 5
            || !policy.allowed
            || cancelled.is_some()
        {
            return Err(Error::InvalidMessage);
        }
    }
    let resumed = clients[0]
        .begin(2, "again", 5000, Instant::now())
        .map_err(|_| Error::InvalidMessage)?;
    first.write_all(resumed, &stop, deadline)?;
    let Event::Reply {
        request: 2,
        outcome: Outcome::Result(text),
        ..
    } = read_event(
        &mut first,
        &mut clients[0],
        &mut inboxes[0],
        &stop,
        deadline,
    )?
    else {
        return Err(Error::InvalidMessage);
    };
    if text != "再開" {
        return Err(Error::InvalidMessage);
    }
    Ok(())
}

fn verify_ready(
    pipe: &mut Pipe,
    client: &mut ClientSession,
    inbox: &mut Inbox,
    stop: &Stop,
    deadline: Instant,
) -> Result<()> {
    let Event::Ready(policy) = read_event(pipe, client, inbox, stop, deadline)? else {
        return Err(Error::InvalidMessage);
    };
    if policy.settings != 1 || policy.authorization != 1 || policy.allowed {
        return Err(Error::InvalidMessage);
    }
    Ok(())
}
