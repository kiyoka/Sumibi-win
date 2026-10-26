use super::*;
use crate::job::{read_event, Inbox};
use sumibi_protocol::session::{ClientSession, Event as ClientEvent, Outcome};

/// Fixed two-client child-process probe for the setup executable. Both client
/// pipes authenticate the same launched Host and carry no external source.
pub fn probe(path: &std::path::Path) -> Result<()> {
    for scenario in [
        Scenario::Normal,
        Scenario::FailedWrite,
        Scenario::Lagging,
        Scenario::ReadyFailure,
    ] {
        for reversed in [false, true] {
            diagnostic_two_client_probe_once(path, reversed, scenario)?;
        }
    }
    Ok(())
}

fn diagnostic_two_client_probe_once(
    path: &std::path::Path,
    reversed: bool,
    scenario: Scenario,
) -> Result<()> {
    use crate::bootstrap::Launched;

    let stop = Stop::new()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let mode = scenario.mode();
    let (child, mut first, mut second, boot, name) =
        Launched::connect_fanout(path, mode, &stop, deadline)?;
    if reversed {
        // The first logical client uses the second opened pipe instance.
        // The Broker must route by Hello, not by connection creation order.
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
    if scenario == Scenario::ReadyFailure {
        return probe_ready_failure(first, second, clients, inboxes, &stop, deadline);
    }
    for (index, client) in clients.iter_mut().enumerate() {
        let pipe = if index == 0 { &mut first } else { &mut second };
        let inbox = &mut inboxes[index];
        let ClientEvent::Ready(policy) = read_event(pipe, client, inbox, &stop, deadline)? else {
            return Err(Error::InvalidMessage);
        };
        if policy.settings != 1 || policy.authorization != 1 || !policy.allowed {
            return Err(Error::InvalidMessage);
        }
    }
    if scenario == Scenario::FailedWrite {
        let reconnect = ReconnectTarget {
            child: &child,
            name: &name,
            boot: boot.id(),
        };
        return probe_failed_notice(first, second, clients, inboxes, &reconnect, &stop, deadline);
    }
    if scenario == Scenario::Lagging {
        return probe_lagging_notice(first, second, clients, inboxes, &stop, deadline);
    }
    let expiring = clients[0]
        .begin(1, "nihongo", 100, Instant::now())
        .map_err(|_| Error::InvalidMessage)?;
    first.write_all(expiring, &stop, deadline)?;
    let independent = clients[1]
        .begin(1, "nihongo", 5000, Instant::now())
        .map_err(|_| Error::InvalidMessage)?;
    second.write_all(independent, &stop, deadline)?;
    let ClientEvent::Reply {
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
    if text != "継続" || clients[0].expire(Instant::now()) != Some(1) {
        return Err(Error::InvalidMessage);
    }
    let old = clients[0]
        .begin(2, "nihongo", 5000, Instant::now())
        .map_err(|_| Error::InvalidMessage)?;
    first.write_all(old, &stop, deadline)?;
    for (index, client) in clients.iter_mut().enumerate() {
        let pipe = if index == 0 { &mut first } else { &mut second };
        let inbox = &mut inboxes[index];
        let ClientEvent::PolicyChanged { policy, cancelled } =
            read_event(pipe, client, inbox, &stop, deadline)?
        else {
            return Err(Error::InvalidMessage);
        };
        if policy.authorization != 2 || policy.allowed || cancelled != (index == 0).then_some(2) {
            return Err(Error::InvalidMessage);
        }
    }
    if clients[0].pending_request().is_some() {
        return Err(Error::InvalidMessage);
    }
    for (index, client) in clients.iter_mut().enumerate() {
        let pipe = if index == 0 { &mut first } else { &mut second };
        let inbox = &mut inboxes[index];
        let ClientEvent::PolicyChanged { policy, cancelled } =
            read_event(pipe, client, inbox, &stop, deadline)?
        else {
            return Err(Error::InvalidMessage);
        };
        if policy.authorization != 3 || !policy.allowed || cancelled.is_some() {
            return Err(Error::InvalidMessage);
        }
    }
    let current = clients[1]
        .begin(2, "nihongo", 5000, Instant::now())
        .map_err(|_| Error::InvalidMessage)?;
    second.write_all(current, &stop, deadline)?;
    let ClientEvent::Reply {
        request: 2,
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
    if text != "日本語" {
        return Err(Error::InvalidMessage);
    }
    let pending = clients[0]
        .begin(3, "nihongo", 5000, Instant::now())
        .map_err(|_| Error::InvalidMessage)?;
    first.write_all(pending, &stop, deadline)?;
    let cancellable = clients[1]
        .begin(3, "nihongo", 5000, Instant::now())
        .map_err(|_| Error::InvalidMessage)?;
    second.write_all(cancellable, &stop, deadline)?;
    let cancel = clients[1]
        .cancel()
        .map_err(|_| Error::InvalidMessage)?
        .ok_or(Error::InvalidMessage)?;
    second.write_all(cancel, &stop, deadline)?;
    let following = clients[1]
        .begin(4, "nihongo", 5000, Instant::now())
        .map_err(|_| Error::InvalidMessage)?;
    second.write_all(following, &stop, deadline)?;
    let ClientEvent::Reply {
        request: 4,
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
    if text != "継続" || clients[0].pending_request() != Some(3) {
        return Err(Error::InvalidMessage);
    }
    let invalid = Envelope {
        version: VERSION,
        session: Id::new(2).ok_or(Error::InvalidMessage)?,
        generation: 1,
        message: Message::Cancel(Cancel { request: 5 }),
    };
    second.write_all(
        message::encode(&invalid).map_err(|_| Error::InvalidMessage)?,
        &stop,
        deadline,
    )?;
    let ClientEvent::PolicyChanged { policy, cancelled } = read_event(
        &mut first,
        &mut clients[0],
        &mut inboxes[0],
        &stop,
        deadline,
    )?
    else {
        return Err(Error::InvalidMessage);
    };
    if policy.authorization != 4 || policy.allowed || cancelled != Some(3) {
        return Err(Error::InvalidMessage);
    }
    let closed = second
        .start_read(&stop, deadline)
        .and_then(|operation| operation.wait())
        .err();
    if !matches!(closed, Some(Error::Disconnected | Error::Closed)) {
        return Err(Error::InvalidMessage);
    }
    Ok(())
}

fn probe_ready_failure(
    mut first: Pipe,
    second: Pipe,
    mut clients: [ClientSession; 2],
    mut inboxes: [Inbox; 2],
    stop: &Stop,
    deadline: Instant,
) -> Result<()> {
    let ClientEvent::Ready(policy) =
        read_event(&mut first, &mut clients[0], &mut inboxes[0], stop, deadline)?
    else {
        return Err(Error::InvalidMessage);
    };
    if policy.authorization != 1 || !policy.allowed {
        return Err(Error::InvalidMessage);
    }
    drop(second);
    clients[1].disconnect();
    let request = clients[0]
        .begin(1, "nihongo", 5000, Instant::now())
        .map_err(|_| Error::InvalidMessage)?;
    first.write_all(request, stop, deadline)?;
    let ClientEvent::Reply {
        request: 1,
        outcome: Outcome::Result(text),
        ..
    } = read_event(&mut first, &mut clients[0], &mut inboxes[0], stop, deadline)?
    else {
        return Err(Error::InvalidMessage);
    };
    if text != "日本語" {
        return Err(Error::InvalidMessage);
    }
    Ok(())
}

fn probe_lagging_notice(
    mut first: Pipe,
    mut second: Pipe,
    mut clients: [ClientSession; 2],
    mut inboxes: [Inbox; 2],
    stop: &Stop,
    deadline: Instant,
) -> Result<()> {
    let old = clients[0]
        .begin(1, "nihongo", 5000, Instant::now())
        .map_err(|_| Error::InvalidMessage)?;
    first.write_all(old, stop, deadline)?;
    let ClientEvent::PolicyChanged { policy, cancelled } =
        read_event(&mut first, &mut clients[0], &mut inboxes[0], stop, deadline)?
    else {
        return Err(Error::InvalidMessage);
    };
    if policy.authorization != 2 || policy.allowed || cancelled != Some(1) {
        return Err(Error::InvalidMessage);
    }
    let ClientEvent::PolicyChanged { policy, cancelled } =
        read_event(&mut first, &mut clients[0], &mut inboxes[0], stop, deadline)?
    else {
        return Err(Error::InvalidMessage);
    };
    if policy.authorization != 3 || !policy.allowed || cancelled.is_some() {
        return Err(Error::InvalidMessage);
    }
    let closed = second
        .start_read(stop, deadline)
        .and_then(|operation| operation.wait())
        .err();
    if !matches!(closed, Some(Error::Disconnected | Error::Closed)) {
        return Err(Error::InvalidMessage);
    }
    let next = clients[0]
        .begin(2, "nihongo", 5000, Instant::now())
        .map_err(|_| Error::InvalidMessage)?;
    first.write_all(next, stop, deadline)?;
    let ClientEvent::Reply {
        request: 2,
        outcome: Outcome::Result(text),
        ..
    } = read_event(&mut first, &mut clients[0], &mut inboxes[0], stop, deadline)?
    else {
        return Err(Error::InvalidMessage);
    };
    if text != "日本語" {
        return Err(Error::InvalidMessage);
    }
    Ok(())
}

struct ReconnectTarget<'a> {
    child: &'a crate::bootstrap::Launched,
    name: &'a crate::support::Nonce,
    boot: Id,
}

fn probe_failed_notice(
    mut first: Pipe,
    mut second: Pipe,
    mut clients: [ClientSession; 2],
    mut inboxes: [Inbox; 2],
    reconnect: &ReconnectTarget<'_>,
    stop: &Stop,
    deadline: Instant,
) -> Result<()> {
    let initial = clients[1]
        .begin(1, "nihongo", 5000, Instant::now())
        .map_err(|_| Error::InvalidMessage)?;
    second.write_all(initial, stop, deadline)?;
    let ClientEvent::Reply {
        request: 1,
        outcome: Outcome::Result(text),
        ..
    } = read_event(
        &mut second,
        &mut clients[1],
        &mut inboxes[1],
        stop,
        deadline,
    )?
    else {
        return Err(Error::InvalidMessage);
    };
    if text != "初回" {
        return Err(Error::InvalidMessage);
    }
    let abandoned = clients[1]
        .begin(2, "nihongo", 5000, Instant::now())
        .map_err(|_| Error::InvalidMessage)?;
    second.write_all(abandoned, stop, deadline)?;
    drop(second);
    let old = clients[0]
        .begin(1, "nihongo", 5000, Instant::now())
        .map_err(|_| Error::InvalidMessage)?;
    first.write_all(old, stop, deadline)?;
    let ClientEvent::PolicyChanged { policy, cancelled } =
        read_event(&mut first, &mut clients[0], &mut inboxes[0], stop, deadline)?
    else {
        return Err(Error::InvalidMessage);
    };
    if policy.authorization != 2 || policy.allowed || cancelled != Some(1) {
        return Err(Error::InvalidMessage);
    }
    let ClientEvent::PolicyChanged { policy, cancelled } =
        read_event(&mut first, &mut clients[0], &mut inboxes[0], stop, deadline)?
    else {
        return Err(Error::InvalidMessage);
    };
    if policy.authorization != 3 || !policy.allowed || cancelled.is_some() {
        return Err(Error::InvalidMessage);
    }
    clients[1].disconnect();
    let mut second = Pipe::connect(reconnect.name, reconnect.child.peer()?)?;
    second.write_all(
        clients[1]
            .reconnect(reconnect.boot)
            .map_err(|_| Error::InvalidMessage)?,
        stop,
        deadline,
    )?;
    let ClientEvent::Ready(policy) = read_event(
        &mut second,
        &mut clients[1],
        &mut inboxes[1],
        stop,
        deadline,
    )?
    else {
        return Err(Error::InvalidMessage);
    };
    if policy.authorization != 3 || !policy.allowed {
        return Err(Error::InvalidMessage);
    }
    let resumed = clients[1]
        .begin(3, "nihongo", 5000, Instant::now())
        .map_err(|_| Error::InvalidMessage)?;
    second.write_all(resumed, stop, deadline)?;
    let ClientEvent::Reply {
        request: 3,
        outcome: Outcome::Result(text),
        ..
    } = read_event(
        &mut second,
        &mut clients[1],
        &mut inboxes[1],
        stop,
        deadline,
    )?
    else {
        return Err(Error::InvalidMessage);
    };
    if text != "再接続" {
        return Err(Error::InvalidMessage);
    }
    let next = clients[0]
        .begin(2, "nihongo", 5000, Instant::now())
        .map_err(|_| Error::InvalidMessage)?;
    first.write_all(next, stop, deadline)?;
    let ClientEvent::Reply {
        request: 2,
        outcome: Outcome::Result(text),
        ..
    } = read_event(&mut first, &mut clients[0], &mut inboxes[0], stop, deadline)?
    else {
        return Err(Error::InvalidMessage);
    };
    if text != "日本語" {
        return Err(Error::InvalidMessage);
    }
    Ok(())
}
