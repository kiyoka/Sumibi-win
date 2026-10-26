use super::*;

/// Two authenticated pipe instances in one child Broker. This fixed fixture
/// exercises real multi-connection I/O without product discovery or HTTP.
pub(crate) fn serve_fanout(bootstrap: Bootstrap, diagnostic_enabled: bool) -> Result<()> {
    let scenario = match bootstrap.mode {
        Mode::DiagnosticFixed { delay_ms: 0 } if diagnostic_enabled => Scenario::Normal,
        Mode::DiagnosticFanoutFailedNotice if diagnostic_enabled => Scenario::FailedWrite,
        Mode::DiagnosticFanoutLaggingNotice if diagnostic_enabled => Scenario::Lagging,
        Mode::DiagnosticFanoutReadyFailure if diagnostic_enabled => Scenario::ReadyFailure,
        Mode::DiagnosticFanoutBadHello if diagnostic_enabled => Scenario::BadHello,
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
    if matches!(scenario, Scenario::ReadyFailure | Scenario::BadHello) {
        return serve_second_handshake_failure(
            bootstrap,
            gate,
            order,
            [(first, first_hello), (second, second_hello)],
            scenario,
            deadline,
        );
    }
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
    // Let the late worker completion reach Fanout before its timer callback.
    // This race must consume only the expired request and keep the pipe alive.
    if order
        .result(
            1,
            expiring.request,
            expiring.revision,
            "期限切れ".into(),
            Instant::now(),
        )
        .map_err(map_fanout_error)?
        .is_some()
        || order
            .expire(1, Instant::now())
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

fn serve_second_handshake_failure(
    bootstrap: Bootstrap,
    gate: BrokerPolicyGate,
    mut order: PolicyFanout,
    connections: [(Pipe, Envelope); 2],
    scenario: Scenario,
    deadline: Instant,
) -> Result<()> {
    let [(mut first, first_hello), (mut second, second_hello)] = connections;
    let ready = order
        .attach(1, first_hello, Instant::now())
        .map_err(map_fanout_error)?;
    first.write_all(
        message::encode(&ready).map_err(|_| Error::InvalidMessage)?,
        &bootstrap.stop,
        deadline,
    )?;
    order.ready_delivered(1).map_err(map_fanout_error)?;
    let request = read_message(&mut first, &bootstrap.stop, deadline)?;
    let BrokerEvent::Convert(convert) = order
        .receive(1, request, Instant::now())
        .map_err(map_fanout_error)?
    else {
        return Err(Error::InvalidMessage);
    };
    gate.authorize(convert.revision)?;
    match scenario {
        Scenario::ReadyFailure => {
            let ready = order
                .attach(2, second_hello, Instant::now())
                .map_err(map_fanout_error)?;
            if !matches!(
                second
                    .write_all(
                        message::encode(&ready).map_err(|_| Error::InvalidMessage)?,
                        &bootstrap.stop,
                        deadline,
                    )
                    .err(),
                Some(Error::Disconnected)
            ) || !order.detach(2)
            {
                return Err(Error::InvalidMessage);
            }
        }
        Scenario::BadHello => {
            if !matches!(
                order.attach(2, second_hello, Instant::now()),
                Err(FanoutError::Broker(BrokerError::WrongBroker))
            ) {
                return Err(Error::InvalidMessage);
            }
        }
        _ => return Err(Error::InvalidMessage),
    }
    if order.active_count() != 1 {
        return Err(Error::InvalidMessage);
    }
    drop(second);
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
    let mut frames = [None, None];
    for (key, notice) in notices {
        let slot = match key {
            1 => &mut frames[0],
            2 => &mut frames[1],
            _ => return Err(Error::InvalidMessage),
        };
        if slot.is_some()
            || !matches!(notice.frame.message, Message::PolicyChanged(actual) if actual == policy)
        {
            return Err(Error::InvalidMessage);
        }
        *slot = Some(message::encode(&notice.frame).map_err(|_| Error::InvalidMessage)?);
    }
    // Start both bounded overlapped writes before waiting on either endpoint.
    // Complete whichever is ready first; one slow connection does not impose
    // its wait on the other connection's notification.
    let first_write = first.start_write(
        frames[0].take().ok_or(Error::InvalidMessage)?,
        stop,
        deadline,
    )?;
    let second_write = second.start_write(
        frames[1].take().ok_or(Error::InvalidMessage)?,
        stop,
        deadline,
    )?;
    let ready = crate::wait_any(&[&first_write, &second_write])?;
    if ready == 1 {
        let second_done = second_write.wait()?;
        finish_started_write(second, second_done, stop, deadline)?;
        order
            .notice_delivered(2, policy)
            .map_err(map_fanout_error)?;
        let first_done = first_write.wait()?;
        finish_started_write(first, first_done, stop, deadline)?;
        order
            .notice_delivered(1, policy)
            .map_err(map_fanout_error)?;
    } else {
        let first_done = first_write.wait()?;
        finish_started_write(first, first_done, stop, deadline)?;
        order
            .notice_delivered(1, policy)
            .map_err(map_fanout_error)?;
        let second_done = second_write.wait()?;
        finish_started_write(second, second_done, stop, deadline)?;
        order
            .notice_delivered(2, policy)
            .map_err(map_fanout_error)?;
    }
    Ok(())
}

fn finish_started_write(
    pipe: &mut Pipe,
    completion: crate::Completion,
    stop: &Stop,
    deadline: Instant,
) -> Result<()> {
    if completion.transferred < completion.bytes.len() {
        let mut remaining = completion.bytes;
        remaining.drain(..completion.transferred);
        pipe.write_all(remaining, stop, deadline)?;
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
