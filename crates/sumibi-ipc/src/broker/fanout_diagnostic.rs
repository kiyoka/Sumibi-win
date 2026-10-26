//! Disposable two-client Broker transport diagnostic.
use super::*;

/// Two authenticated pipe instances in one child Broker. This fixed fixture
/// exercises real multi-connection I/O without product discovery or HTTP.
pub(super) fn serve_fanout(bootstrap: Bootstrap, diagnostic_enabled: bool) -> Result<()> {
    if !diagnostic_enabled || bootstrap.mode != (Mode::DiagnosticFixed { delay_ms: 0 }) {
        return Err(Error::PolicyDenied);
    }
    let first = Server::create_pair(&bootstrap.name)?;
    let second = first.additional()?;
    if first.additional().is_ok() {
        return Err(Error::InvalidMessage);
    }
    bootstrap.ready()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let first_peer = crate::PinnedProcess::from_launch_handle(bootstrap.parent.as_handle())?;
    let second_peer = crate::PinnedProcess::from_launch_handle(bootstrap.parent.as_handle())?;
    let mut first = first.accept(first_peer, &bootstrap.stop, deadline)?;
    let mut second = second.accept(second_peer, &bootstrap.stop, deadline)?;
    let mut first_hello = read_message(&mut first, &bootstrap.stop, deadline)?;
    let mut second_hello = read_message(&mut second, &bootstrap.stop, deadline)?;
    // Windows may assign CreateFile calls to available instances in either
    // order. Route by the authenticated connection's Hello, not creation order.
    let first_id = Id::new(1).ok_or(Error::InvalidMessage)?;
    let second_id = Id::new(2).ok_or(Error::InvalidMessage)?;
    if first_hello.session == second_id && second_hello.session == first_id {
        std::mem::swap(&mut first, &mut second);
        std::mem::swap(&mut first_hello, &mut second_hello);
    }
    if first_hello.session != first_id || second_hello.session != second_id {
        return Err(Error::InvalidMessage);
    }
    let policy = Policy {
        settings: 1,
        authorization: 1,
        allowed: true,
    };
    let gate = BrokerPolicyGate::new(policy)?;
    let mut order = PolicyFanout::new(bootstrap.boot.id(), policy).map_err(map_fanout_error)?;
    for (key, pipe, hello) in [(1, &mut first, first_hello), (2, &mut second, second_hello)] {
        let ready = order
            .attach(key, hello, Instant::now())
            .map_err(map_fanout_error)?;
        pipe.write_all(
            message::encode(&ready).map_err(|_| Error::InvalidMessage)?,
            &bootstrap.stop,
            deadline,
        )?;
        order.ready_delivered(key).map_err(map_fanout_error)?;
    }
    let request = read_message(&mut first, &bootstrap.stop, deadline)?;
    let BrokerEvent::Convert(convert) = order
        .receive(1, request, Instant::now())
        .map_err(map_fanout_error)?
    else {
        return Err(Error::InvalidMessage);
    };
    gate.authorize(convert.revision)?;
    let revoked = Policy {
        authorization: 2,
        allowed: false,
        ..policy
    };
    if !gate.publish(&mut order, revoked)?.is_empty() {
        return Err(Error::InvalidMessage);
    }
    let old_result = order
        .result(
            1,
            convert.request,
            convert.revision,
            "旧結果".into(),
            Instant::now(),
        )
        .map_err(map_fanout_error)?;
    if old_result.is_some() {
        return Err(Error::InvalidMessage);
    }
    write_fanout_notices(
        &mut order,
        &mut first,
        &mut second,
        &bootstrap.stop,
        deadline,
        revoked,
    )?;
    let regranted = Policy {
        authorization: 3,
        allowed: true,
        ..policy
    };
    if !gate.publish(&mut order, regranted)?.is_empty() {
        return Err(Error::InvalidMessage);
    }
    write_fanout_notices(
        &mut order,
        &mut first,
        &mut second,
        &bootstrap.stop,
        deadline,
        regranted,
    )?;
    let next = read_message(&mut second, &bootstrap.stop, deadline)?;
    let BrokerEvent::Convert(convert) = order
        .receive(2, next, Instant::now())
        .map_err(map_fanout_error)?
    else {
        return Err(Error::InvalidMessage);
    };
    gate.authorize(convert.revision)?;
    let result = order
        .result(
            2,
            convert.request,
            convert.revision,
            "日本語".into(),
            Instant::now(),
        )
        .map_err(map_fanout_error)?
        .ok_or(Error::InvalidMessage)?;
    let publication = gate.authorize_for_publish(convert.revision)?;
    second.write_all(
        message::encode(&result).map_err(|_| Error::InvalidMessage)?,
        &bootstrap.stop,
        deadline,
    )?;
    drop(publication);
    let still_live = read_message(&mut first, &bootstrap.stop, deadline)?;
    let BrokerEvent::Convert(convert) = order
        .receive(1, still_live, Instant::now())
        .map_err(map_fanout_error)?
    else {
        return Err(Error::InvalidMessage);
    };
    gate.authorize(convert.revision)?;
    let invalid = read_message(&mut second, &bootstrap.stop, deadline)?;
    if !matches!(
        order.receive(2, invalid, Instant::now()),
        Err(FanoutError::Broker(BrokerError::InvalidMessage))
    ) || order.active_count() != 1
    {
        return Err(Error::InvalidMessage);
    }
    drop(second);
    let final_policy = Policy {
        authorization: 4,
        allowed: false,
        ..policy
    };
    if !gate.publish(&mut order, final_policy)?.is_empty() {
        return Err(Error::InvalidMessage);
    }
    let mut notices = order.pending_writes();
    if notices.len() != 1 {
        return Err(Error::InvalidMessage);
    }
    let (key, notice) = notices.pop().ok_or(Error::InvalidMessage)?;
    if key != 1 || notice.cancelled != Some(convert.request) {
        return Err(Error::InvalidMessage);
    }
    first.write_all(
        message::encode(&notice.frame).map_err(|_| Error::InvalidMessage)?,
        &bootstrap.stop,
        deadline,
    )?;
    order
        .notice_delivered(1, final_policy)
        .map_err(map_fanout_error)?;
    let _ = wait_stop(&bootstrap.stop, 10_000)?;
    Ok(())
}

fn write_fanout_notices(
    order: &mut PolicyFanout,
    first: &mut Pipe,
    second: &mut Pipe,
    stop: &Stop,
    deadline: Instant,
    policy: Policy,
) -> Result<()> {
    let notices = order.pending_writes();
    if notices.len() != 2 {
        return Err(Error::InvalidMessage);
    }
    for (key, notice) in notices {
        let pipe = match key {
            1 => &mut *first,
            2 => &mut *second,
            _ => return Err(Error::InvalidMessage),
        };
        if !matches!(notice.frame.message, Message::PolicyChanged(actual) if actual == policy) {
            return Err(Error::InvalidMessage);
        }
        pipe.write_all(
            message::encode(&notice.frame).map_err(|_| Error::InvalidMessage)?,
            stop,
            deadline,
        )?;
        order
            .notice_delivered(key, policy)
            .map_err(map_fanout_error)?;
    }
    Ok(())
}

/// Fixed two-client child-process probe for the setup executable. Both client
/// pipes authenticate the same launched Host and carry no external source.
pub fn probe(path: &std::path::Path) -> Result<()> {
    for reversed in [false, true] {
        diagnostic_two_client_probe_once(path, reversed)?;
    }
    Ok(())
}

fn diagnostic_two_client_probe_once(path: &std::path::Path, reversed: bool) -> Result<()> {
    use crate::bootstrap::Launched;
    use crate::job::{read_event, Inbox};
    use sumibi_protocol::session::{ClientSession, Event as ClientEvent, Outcome};

    let stop = Stop::new()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let (_child, mut first, mut second, boot) = Launched::connect_fanout(path, &stop, deadline)?;
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
    let old = clients[0]
        .begin(1, "nihongo", 5000, Instant::now())
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
        if policy.authorization != 2 || policy.allowed || cancelled != (index == 0).then_some(1) {
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
    if text != "日本語" {
        return Err(Error::InvalidMessage);
    }
    let pending = clients[0]
        .begin(3, "nihongo", 5000, Instant::now())
        .map_err(|_| Error::InvalidMessage)?;
    first.write_all(pending, &stop, deadline)?;
    let invalid = Envelope {
        version: VERSION,
        session: Id::new(2).ok_or(Error::InvalidMessage)?,
        generation: 1,
        message: Message::Cancel(Cancel { request: 2 }),
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
    if policy.authorization != 4 || policy.allowed || cancelled != Some(2) {
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
