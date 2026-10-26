//! Disposable two-client Broker transport diagnostic.
use super::*;
use crate::job::{read_event, Inbox};
use sumibi_protocol::session::{ClientSession, Event as ClientEvent, Outcome};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scenario {
    Normal,
    FailedWrite,
    Lagging,
}

impl Scenario {
    fn mode(self) -> Mode {
        match self {
            Self::Normal => Mode::DiagnosticFixed { delay_ms: 0 },
            Self::FailedWrite => Mode::DiagnosticFanoutFailedNotice,
            Self::Lagging => Mode::DiagnosticFanoutLaggingNotice,
        }
    }
}

/// Two authenticated pipe instances in one child Broker. This fixed fixture
/// exercises real multi-connection I/O without product discovery or HTTP.
pub(super) fn serve_fanout(bootstrap: Bootstrap, diagnostic_enabled: bool) -> Result<()> {
    let scenario = match bootstrap.mode {
        Mode::DiagnosticFixed { delay_ms: 0 } if diagnostic_enabled => Scenario::Normal,
        Mode::DiagnosticFanoutFailedNotice if diagnostic_enabled => Scenario::FailedWrite,
        Mode::DiagnosticFanoutLaggingNotice if diagnostic_enabled => Scenario::Lagging,
        _ => return Err(Error::PolicyDenied),
    };
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
    if scenario == Scenario::FailedWrite {
        return serve_failed_notice(bootstrap, gate, order, first, second, deadline);
    }
    if scenario == Scenario::Lagging {
        return serve_lagging_notice(bootstrap, gate, order, first, second, deadline);
    }
    let expiring = read_message(&mut first, &bootstrap.stop, deadline)?;
    let BrokerEvent::Convert(expiring) = order
        .receive(1, expiring, Instant::now())
        .map_err(map_fanout_error)?
    else {
        return Err(Error::InvalidMessage);
    };
    gate.authorize(expiring.revision)?;
    let independent = read_message(&mut second, &bootstrap.stop, deadline)?;
    let BrokerEvent::Convert(independent) = order
        .receive(2, independent, Instant::now())
        .map_err(map_fanout_error)?
    else {
        return Err(Error::InvalidMessage);
    };
    gate.authorize(independent.revision)?;
    if wait_stop(&bootstrap.stop, 150)? {
        return Err(Error::Cancelled);
    }
    if order.expire(1, Instant::now()).map_err(map_fanout_error)? != Some(expiring.request)
        || order
            .result(
                1,
                expiring.request,
                expiring.revision,
                "期限切れ".into(),
                Instant::now(),
            )
            .map_err(map_fanout_error)?
            .is_some()
    {
        return Err(Error::InvalidMessage);
    }
    let result = order
        .result(
            2,
            independent.request,
            independent.revision,
            "継続".into(),
            Instant::now(),
        )
        .map_err(map_fanout_error)?
        .ok_or(Error::InvalidMessage)?;
    let publication = gate.authorize_for_publish(independent.revision)?;
    second.write_all(
        message::encode(&result).map_err(|_| Error::InvalidMessage)?,
        &bootstrap.stop,
        deadline,
    )?;
    drop(publication);
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
    let mut inbox = BrokerInbox::default();
    let cancellable = inbox.read(&mut second, &bootstrap.stop, deadline)?;
    let BrokerEvent::Convert(cancellable) = order
        .receive(2, cancellable, Instant::now())
        .map_err(map_fanout_error)?
    else {
        return Err(Error::InvalidMessage);
    };
    gate.authorize(cancellable.revision)?;
    let cancel = inbox.read(&mut second, &bootstrap.stop, deadline)?;
    if !matches!(
        order.receive(2, cancel, Instant::now()),
        Ok(BrokerEvent::Cancelled(request)) if request == cancellable.request
    ) {
        return Err(Error::InvalidMessage);
    }
    if order
        .result(
            2,
            cancellable.request,
            cancellable.revision,
            "取消済み".into(),
            Instant::now(),
        )
        .map_err(map_fanout_error)?
        .is_some()
    {
        return Err(Error::InvalidMessage);
    }
    let following = inbox.read(&mut second, &bootstrap.stop, deadline)?;
    let BrokerEvent::Convert(following) = order
        .receive(2, following, Instant::now())
        .map_err(map_fanout_error)?
    else {
        return Err(Error::InvalidMessage);
    };
    gate.authorize(following.revision)?;
    let result = order
        .result(
            2,
            following.request,
            following.revision,
            "継続".into(),
            Instant::now(),
        )
        .map_err(map_fanout_error)?
        .ok_or(Error::InvalidMessage)?;
    let publication = gate.authorize_for_publish(following.revision)?;
    second.write_all(
        message::encode(&result).map_err(|_| Error::InvalidMessage)?,
        &bootstrap.stop,
        deadline,
    )?;
    drop(publication);
    let invalid = inbox.read(&mut second, &bootstrap.stop, deadline)?;
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

fn serve_lagging_notice(
    bootstrap: Bootstrap,
    gate: BrokerPolicyGate,
    mut order: PolicyFanout,
    mut first: Pipe,
    second: Pipe,
    deadline: Instant,
) -> Result<()> {
    let request = read_message(&mut first, &bootstrap.stop, deadline)?;
    let BrokerEvent::Convert(convert) = order
        .receive(1, request, Instant::now())
        .map_err(map_fanout_error)?
    else {
        return Err(Error::InvalidMessage);
    };
    gate.authorize(convert.revision)?;
    let initial = gate.snapshot()?;
    let revoked = Policy {
        authorization: initial.authorization + 1,
        allowed: false,
        ..initial
    };
    if !gate.publish(&mut order, revoked)?.is_empty() {
        return Err(Error::InvalidMessage);
    }
    if order
        .result(
            1,
            convert.request,
            convert.revision,
            "旧結果".into(),
            Instant::now(),
        )
        .map_err(map_fanout_error)?
        .is_some()
    {
        return Err(Error::InvalidMessage);
    }
    let notices = order.pending_writes();
    if notices.len() != 2 {
        return Err(Error::InvalidMessage);
    }
    for (key, notice) in notices {
        match key {
            1 if notice.cancelled == Some(convert.request) => {
                first.write_all(
                    message::encode(&notice.frame).map_err(|_| Error::InvalidMessage)?,
                    &bootstrap.stop,
                    deadline,
                )?;
                order
                    .notice_delivered(1, revoked)
                    .map_err(map_fanout_error)?;
            }
            2 if notice.cancelled.is_none() => {
                // A deliberately unwritable subscriber keeps one claimed
                // notice. The next epoch must retire only this transport.
            }
            _ => return Err(Error::InvalidMessage),
        }
    }
    let regranted = Policy {
        authorization: revoked.authorization + 1,
        allowed: true,
        ..revoked
    };
    if gate.publish(&mut order, regranted)? != [2] {
        return Err(Error::InvalidMessage);
    }
    drop(second);
    if order.active_count() != 1 {
        return Err(Error::InvalidMessage);
    }
    let mut notices = order.pending_writes();
    if notices.len() != 1 {
        return Err(Error::InvalidMessage);
    }
    let (key, notice) = notices.pop().ok_or(Error::InvalidMessage)?;
    if key != 1 || notice.cancelled.is_some() {
        return Err(Error::InvalidMessage);
    }
    first.write_all(
        message::encode(&notice.frame).map_err(|_| Error::InvalidMessage)?,
        &bootstrap.stop,
        deadline,
    )?;
    order
        .notice_delivered(1, regranted)
        .map_err(map_fanout_error)?;
    let next = read_message(&mut first, &bootstrap.stop, deadline)?;
    let BrokerEvent::Convert(convert) = order
        .receive(1, next, Instant::now())
        .map_err(map_fanout_error)?
    else {
        return Err(Error::InvalidMessage);
    };
    gate.authorize(convert.revision)?;
    let result = order
        .result(
            1,
            convert.request,
            convert.revision,
            "日本語".into(),
            Instant::now(),
        )
        .map_err(map_fanout_error)?
        .ok_or(Error::InvalidMessage)?;
    let publication = gate.authorize_for_publish(convert.revision)?;
    first.write_all(
        message::encode(&result).map_err(|_| Error::InvalidMessage)?,
        &bootstrap.stop,
        deadline,
    )?;
    drop(publication);
    let _ = wait_stop(&bootstrap.stop, 10_000)?;
    Ok(())
}

fn serve_failed_notice(
    bootstrap: Bootstrap,
    gate: BrokerPolicyGate,
    mut order: PolicyFanout,
    mut first: Pipe,
    mut second: Pipe,
    deadline: Instant,
) -> Result<()> {
    let initial_request = read_message(&mut second, &bootstrap.stop, deadline)?;
    let BrokerEvent::Convert(initial_request) = order
        .receive(2, initial_request, Instant::now())
        .map_err(map_fanout_error)?
    else {
        return Err(Error::InvalidMessage);
    };
    gate.authorize(initial_request.revision)?;
    let initial_result = order
        .result(
            2,
            initial_request.request,
            initial_request.revision,
            "初回".into(),
            Instant::now(),
        )
        .map_err(map_fanout_error)?
        .ok_or(Error::InvalidMessage)?;
    let publication = gate.authorize_for_publish(initial_request.revision)?;
    second.write_all(
        message::encode(&initial_result).map_err(|_| Error::InvalidMessage)?,
        &bootstrap.stop,
        deadline,
    )?;
    drop(publication);
    let abandoned = read_message(&mut second, &bootstrap.stop, deadline)?;
    let BrokerEvent::Convert(abandoned) = order
        .receive(2, abandoned, Instant::now())
        .map_err(map_fanout_error)?
    else {
        return Err(Error::InvalidMessage);
    };
    gate.authorize(abandoned.revision)?;
    let request = read_message(&mut first, &bootstrap.stop, deadline)?;
    let BrokerEvent::Convert(convert) = order
        .receive(1, request, Instant::now())
        .map_err(map_fanout_error)?
    else {
        return Err(Error::InvalidMessage);
    };
    gate.authorize(convert.revision)?;
    // The parent closes its second handle before sending the first client's
    // request. Keep the stale Fanout entry so the notice reaches WriteFile.
    let initial = gate.snapshot()?;
    let revoked = Policy {
        authorization: initial.authorization + 1,
        allowed: false,
        ..initial
    };
    if !gate.publish(&mut order, revoked)?.is_empty() {
        return Err(Error::InvalidMessage);
    }
    if order
        .result(
            1,
            convert.request,
            convert.revision,
            "旧結果".into(),
            Instant::now(),
        )
        .map_err(map_fanout_error)?
        .is_some()
    {
        return Err(Error::InvalidMessage);
    }
    if order
        .result(
            2,
            abandoned.request,
            abandoned.revision,
            "切断前の旧結果".into(),
            Instant::now(),
        )
        .map_err(map_fanout_error)?
        .is_some()
    {
        return Err(Error::InvalidMessage);
    }
    let notices = order.pending_writes();
    if notices.len() != 2 {
        return Err(Error::InvalidMessage);
    }
    for (key, notice) in notices {
        let frame = message::encode(&notice.frame).map_err(|_| Error::InvalidMessage)?;
        match key {
            1 if notice.cancelled == Some(convert.request) => {
                first.write_all(frame, &bootstrap.stop, deadline)?;
                order
                    .notice_delivered(1, revoked)
                    .map_err(map_fanout_error)?;
            }
            2 if notice.cancelled == Some(abandoned.request) => {
                let failed = second.write_all(frame, &bootstrap.stop, deadline).err();
                if !matches!(failed, Some(Error::Disconnected)) {
                    return Err(Error::InvalidMessage);
                }
                if !order.detach(2) {
                    return Err(Error::InvalidMessage);
                }
            }
            _ => return Err(Error::InvalidMessage),
        }
    }
    drop(second);
    if order.active_count() != 1 {
        return Err(Error::InvalidMessage);
    }
    let replacement = first.replacement_pair()?;
    let regranted = Policy {
        authorization: revoked.authorization + 1,
        allowed: true,
        ..revoked
    };
    if !gate.publish(&mut order, regranted)?.is_empty() {
        return Err(Error::InvalidMessage);
    }
    let mut notices = order.pending_writes();
    if notices.len() != 1 {
        return Err(Error::InvalidMessage);
    }
    let (key, notice) = notices.pop().ok_or(Error::InvalidMessage)?;
    if key != 1 || notice.cancelled.is_some() {
        return Err(Error::InvalidMessage);
    }
    first.write_all(
        message::encode(&notice.frame).map_err(|_| Error::InvalidMessage)?,
        &bootstrap.stop,
        deadline,
    )?;
    order
        .notice_delivered(1, regranted)
        .map_err(map_fanout_error)?;
    let peer = crate::PinnedProcess::from_launch_handle(bootstrap.parent.as_handle())?;
    let mut second = replacement.accept(peer, &bootstrap.stop, deadline)?;
    let hello = read_message(&mut second, &bootstrap.stop, deadline)?;
    if hello.session != Id::new(2).ok_or(Error::InvalidMessage)? || hello.generation != 2 {
        return Err(Error::InvalidMessage);
    }
    let ready = order
        .attach(3, hello, Instant::now())
        .map_err(map_fanout_error)?;
    second.write_all(
        message::encode(&ready).map_err(|_| Error::InvalidMessage)?,
        &bootstrap.stop,
        deadline,
    )?;
    order.ready_delivered(3).map_err(map_fanout_error)?;
    let resumed = read_message(&mut second, &bootstrap.stop, deadline)?;
    let BrokerEvent::Convert(resumed) = order
        .receive(3, resumed, Instant::now())
        .map_err(map_fanout_error)?
    else {
        return Err(Error::InvalidMessage);
    };
    gate.authorize(resumed.revision)?;
    let result = order
        .result(
            3,
            resumed.request,
            resumed.revision,
            "再接続".into(),
            Instant::now(),
        )
        .map_err(map_fanout_error)?
        .ok_or(Error::InvalidMessage)?;
    let publication = gate.authorize_for_publish(resumed.revision)?;
    second.write_all(
        message::encode(&result).map_err(|_| Error::InvalidMessage)?,
        &bootstrap.stop,
        deadline,
    )?;
    drop(publication);
    let next = read_message(&mut first, &bootstrap.stop, deadline)?;
    let BrokerEvent::Convert(convert) = order
        .receive(1, next, Instant::now())
        .map_err(map_fanout_error)?
    else {
        return Err(Error::InvalidMessage);
    };
    gate.authorize(convert.revision)?;
    let result = order
        .result(
            1,
            convert.request,
            convert.revision,
            "日本語".into(),
            Instant::now(),
        )
        .map_err(map_fanout_error)?
        .ok_or(Error::InvalidMessage)?;
    let publication = gate.authorize_for_publish(convert.revision)?;
    first.write_all(
        message::encode(&result).map_err(|_| Error::InvalidMessage)?,
        &bootstrap.stop,
        deadline,
    )?;
    drop(publication);
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

#[derive(Default)]
struct BrokerInbox {
    decoder: Decoder,
    unread: Vec<u8>,
}

impl BrokerInbox {
    fn read(&mut self, pipe: &mut Pipe, stop: &Stop, deadline: Instant) -> Result<Envelope> {
        loop {
            if !self.unread.is_empty() {
                let decoded = self
                    .decoder
                    .decode(&self.unread)
                    .map_err(|_| Error::InvalidMessage)?;
                self.unread.drain(..decoded.consumed);
                if let Some(body) = decoded.body {
                    return message::decode(body).map_err(|_| Error::InvalidMessage);
                }
            }
            self.unread = pipe.start_read(stop, deadline)?.wait()?.bytes;
        }
    }
}

/// Fixed two-client child-process probe for the setup executable. Both client
/// pipes authenticate the same launched Host and carry no external source.
pub fn probe(path: &std::path::Path) -> Result<()> {
    for reversed in [false, true] {
        diagnostic_two_client_probe_once(path, reversed, Scenario::Normal)?;
    }
    diagnostic_two_client_probe_once(path, false, Scenario::FailedWrite)?;
    diagnostic_two_client_probe_once(path, false, Scenario::Lagging)?;
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
