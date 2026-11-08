//! Initial Chat conversion through ordered native HTTPS I/O.
use crate::decisions::BudgetManager;
use crate::manual::{
    Destination, ManualRequest, OrderedConversionTransport, PolicySource, Scheme, SendAdmission,
    SendError, SendGate, TransportError,
};
use crate::policy::PolicyAuthority;
use std::time::Instant;
use sumibi_core::candidate::CandidateItem;
use sumibi_core::decisions::{DecisionResultItem, DecisionState};
use sumibi_protocol::message::Revision;
use sumibi_winhttp::{Error, PreparedRequest};

/// Resolve the model from the requested policy revision. The same authority
/// rechecks destination, credential and consent before send and after receive.
/// This worker-only entry neither grants consent nor changes stored settings.
pub fn convert_chat_with_policy(
    request: ManualRequest<'_>,
    authority: &PolicyAuthority,
    admission: &impl SendAdmission,
) -> Result<String, SendError> {
    convert_with_exchange(request, authority, admission, &NativeExchange { admission })
}

/// Additional chat conversion with local dictionary completions (ADR 0099, ADR 0100).
/// Requests additional candidates from remote LLM and integrates Nakaguro, Mozc and Sudachi.
/// Fail-closed: remote LLM failure does not perform silent local fallback substitution.
pub fn convert_additional_chat_with_policy(
    request: ManualRequest<'_>,
    current_result: &str,
    authority: &PolicyAuthority,
    admission: &impl SendAdmission,
) -> Result<Vec<CandidateItem>, SendError> {
    convert_additional_with_exchange(
        request,
        current_result,
        authority,
        admission,
        &NativeExchange { admission },
    )
}

/// Evaluate Decisions auto-conversion under separate consent, cost protection and strict schema validation (ADR 0102).
pub fn evaluate_decisions_with_policy(
    destination: &Destination,
    revision: Revision,
    states: &[DecisionState],
    authority: &PolicyAuthority,
    admission: &impl SendAdmission,
    budget: &BudgetManager,
    deadline: Instant,
) -> Result<Vec<DecisionResultItem>, SendError> {
    evaluate_decisions_with_exchange(
        destination,
        revision,
        states,
        authority,
        admission,
        budget,
        deadline,
        &NativeExchange { admission },
    )
}

#[allow(clippy::too_many_arguments)]
fn evaluate_decisions_with_exchange(
    destination: &Destination,
    revision: Revision,
    states: &[DecisionState],
    authority: &PolicyAuthority,
    admission: &impl SendAdmission,
    budget: &BudgetManager,
    deadline: Instant,
    exchange: &dyn Exchange,
) -> Result<Vec<DecisionResultItem>, SendError> {
    if destination.scheme() != Scheme::Https {
        return Err(SendError::InvalidRequest);
    }
    // 1. Separate consent check (ADR 0102)
    if !authority.is_auto_consented() {
        return Err(SendError::PolicyDenied);
    }
    let snapshot = authority.snapshot().ok_or(SendError::PolicyDenied)?;
    if snapshot.settings != revision.settings || snapshot.authorization != revision.authorization {
        return Err(SendError::PolicyDenied);
    }
    let model = snapshot.model.as_deref().ok_or(SendError::PolicyDenied)?;

    // 2. Cost protection pre-allocation (ADR 0102)
    budget.reserve().map_err(|_| SendError::PolicyDenied)?;

    let expected_ids: Vec<u64> = states.iter().map(|s| s.id).collect();
    let body = crate::decisions::request_body_decisions(model, states)
        .map_err(|_| SendError::InvalidRequest)?;

    let transport = DecisionsTransport {
        expected_ids,
        body,
        exchange,
    };

    let json = crate::manual::evaluate_decisions_ordered(
        destination,
        revision,
        deadline,
        authority,
        admission,
        &transport,
    )?;
    serde_json::from_str::<Vec<DecisionResultItem>>(&json).map_err(|_| SendError::InvalidResponse)
}

struct DecisionsTransport<'a> {
    expected_ids: Vec<u64>,
    body: Vec<u8>,
    exchange: &'a dyn Exchange,
}

impl OrderedConversionTransport for DecisionsTransport<'_> {
    fn convert_ordered(
        &self,
        destination: &Destination,
        credential: &str,
        _source: &str,
        deadline: Instant,
        gate: &mut dyn SendGate,
    ) -> Result<String, SendError> {
        let bytes =
            self.exchange
                .exchange(destination, credential, self.body.clone(), deadline, gate)?;
        let items = crate::decisions::response_decisions(&bytes, &self.expected_ids)
            .map_err(|_| SendError::InvalidResponse)?;
        serde_json::to_string(&items).map_err(|_| SendError::InvalidResponse)
    }
}

fn convert_additional_with_exchange(
    request: ManualRequest<'_>,
    current_result: &str,
    authority: &PolicyAuthority,
    admission: &impl SendAdmission,
    exchange: &dyn Exchange,
) -> Result<Vec<CandidateItem>, SendError> {
    if request.destination.scheme() != Scheme::Https {
        return Err(SendError::InvalidRequest);
    }
    let snapshot = authority.snapshot().ok_or(SendError::PolicyDenied)?;
    if snapshot.settings != request.revision.settings
        || snapshot.authorization != request.revision.authorization
    {
        return Err(SendError::PolicyDenied);
    }
    let model = snapshot.model.as_deref().ok_or(SendError::PolicyDenied)?;
    let transport = AdditionalChatTransport {
        model,
        current_result,
        user_dictionary: &snapshot.user_dictionary,
        exchange,
    };
    let json = crate::manual::convert_ordered(request, authority, admission, &transport)?;
    serde_json::from_str::<Vec<CandidateItem>>(&json).map_err(|_| SendError::InvalidResponse)
}

struct AdditionalChatTransport<'a> {
    model: &'a str,
    current_result: &'a str,
    user_dictionary: &'a str,
    exchange: &'a dyn Exchange,
}

impl OrderedConversionTransport for AdditionalChatTransport<'_> {
    fn convert_ordered(
        &self,
        destination: &Destination,
        credential: &str,
        source: &str,
        deadline: Instant,
        gate: &mut dyn SendGate,
    ) -> Result<String, SendError> {
        let input = crate::first_conversion::ConversionInput::with_context(
            source,
            Some(self.current_result),
            Some(self.user_dictionary),
        );
        let body = crate::first_conversion::request_body_additional(self.model, &input)
            .map_err(|_| SendError::InvalidRequest)?;
        let bytes = self
            .exchange
            .exchange(destination, credential, body, deadline, gate)?;
        let llm_candidates =
            crate::first_conversion::response_candidates(&bytes, Some(self.current_result))
                .map_err(|_| SendError::InvalidResponse)?;
        let complete = crate::local_dictionary::build_complete_candidates(
            source,
            Some(self.current_result),
            &llm_candidates,
            crate::first_conversion::MAX_ADDITIONAL_CANDIDATES,
        );
        serde_json::to_string(&complete).map_err(|_| SendError::InvalidResponse)
    }
}

// Injection is private: product callers cannot substitute plaintext or skip
// certificate validation. Unit tests exercise policy/JSON with an in-memory
// exchange; native socket and callback behavior is tested in sumibi-winhttp.
trait Exchange {
    fn exchange(
        &self,
        destination: &Destination,
        credential: &str,
        body: Vec<u8>,
        deadline: Instant,
        gate: &mut dyn SendGate,
    ) -> Result<Vec<u8>, SendError>;
}

struct NativeExchange<'a> {
    admission: &'a dyn SendAdmission,
}
impl Exchange for NativeExchange<'_> {
    fn exchange(
        &self,
        destination: &Destination,
        credential: &str,
        body: Vec<u8>,
        deadline: Instant,
        gate: &mut dyn SendGate,
    ) -> Result<Vec<u8>, SendError> {
        let cancelled = || self.admission.is_cancelled();
        let request = send_body(destination, credential, body, deadline, gate, &cancelled)?;
        receive_body(request, gate, &cancelled)
    }
}

fn convert_with_exchange(
    request: ManualRequest<'_>,
    authority: &PolicyAuthority,
    admission: &impl SendAdmission,
    exchange: &dyn Exchange,
) -> Result<String, SendError> {
    if request.destination.scheme() != Scheme::Https {
        return Err(SendError::InvalidRequest);
    }
    let snapshot = authority.snapshot().ok_or(SendError::PolicyDenied)?;
    if snapshot.settings != request.revision.settings
        || snapshot.authorization != request.revision.authorization
    {
        return Err(SendError::PolicyDenied);
    }
    let model = snapshot.model.as_deref().ok_or(SendError::PolicyDenied)?;
    let transport = ChatTransport {
        model,
        user_dictionary: &snapshot.user_dictionary,
        exchange,
    };
    crate::manual::convert_ordered(request, authority, admission, &transport)
}

struct ChatTransport<'a> {
    model: &'a str,
    user_dictionary: &'a str,
    exchange: &'a dyn Exchange,
}
impl OrderedConversionTransport for ChatTransport<'_> {
    fn convert_ordered(
        &self,
        destination: &Destination,
        credential: &str,
        source: &str,
        deadline: Instant,
        gate: &mut dyn SendGate,
    ) -> Result<String, SendError> {
        let input = crate::first_conversion::ConversionInput::with_context(
            source,
            None,
            Some(self.user_dictionary),
        );
        let body = crate::first_conversion::request_body_with_input(self.model, &input)
            .map_err(|_| SendError::InvalidRequest)?;
        let bytes = self
            .exchange
            .exchange(destination, credential, body, deadline, gate)?;
        crate::first_conversion::response_candidate(&bytes).map_err(|_| SendError::InvalidResponse)
    }
}

pub fn prepare(destination: &Destination) -> Result<PreparedRequest, Error> {
    if destination.scheme() != Scheme::Https {
        return Err(Error::InvalidDestination);
    }
    PreparedRequest::new(
        destination.host(),
        destination.port(),
        destination.base_path(),
    )
}

/// Hold the Host send guard until WinHTTP confirms all initial body bytes
/// were sent. The returned request still owns its buffers and needs response
/// reception or explicit close. No endpoint fallback or retry is performed.
pub fn send_body(
    destination: &Destination,
    credential: &str,
    body: Vec<u8>,
    deadline: Instant,
    gate: &mut dyn SendGate,
    cancelled: &dyn Fn() -> bool,
) -> Result<PreparedRequest, SendError> {
    if gate.is_cancelled() || cancelled() {
        return Err(SendError::PolicyDenied);
    }
    let mut request = prepare(destination).map_err(classify)?;
    let mut body = Some(body);
    let mut sent = false;
    gate.write_authorized(&mut || {
        let body = body.take().ok_or(TransportError::InvalidRequest)?;
        request
            .send(credential, body, deadline, cancelled)
            .map_err(classify)?;
        sent = true;
        Ok(())
    })?;
    if !sent {
        return Err(SendError::InvalidResponse);
    }
    if gate.is_cancelled() || cancelled() {
        return Err(SendError::PolicyDenied);
    }
    Ok(request)
}

/// Receive after releasing the outbound send guard, so policy changes are not
/// blocked by a slow response. The caller must still validate its policy
/// revision and credential identity before accepting a conversion result.
pub fn receive_body(
    request: PreparedRequest,
    gate: &dyn SendGate,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<u8>, SendError> {
    let result = request.receive(&|| gate.is_cancelled() || cancelled());
    if gate.is_cancelled() || cancelled() {
        return Err(SendError::PolicyDenied);
    }
    result.map_err(|error| match error {
        Error::InvalidResponse => SendError::InvalidResponse,
        _ => SendError::Transport(classify(error)),
    })
}

fn classify(error: Error) -> TransportError {
    match error {
        Error::InvalidDestination
        | Error::InvalidPayload
        | Error::InvalidDeadline
        | Error::InvalidState => TransportError::InvalidRequest,
        Error::Timeout => TransportError::Timeout,
        Error::Cancelled => TransportError::Cancelled,
        Error::InvalidResponse => TransportError::InvalidResponse,
        _ => TransportError::Connection,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manual::Credential;
    use std::cell::{Cell, RefCell};
    use std::time::Duration;
    use sumibi_protocol::message::Revision;

    #[derive(Default)]
    struct Admission(Cell<bool>);
    impl SendAdmission for Admission {
        fn is_cancelled(&self) -> bool {
            self.0.get()
        }
        fn write_if_active(
            &self,
            write: &mut dyn FnMut() -> Result<(), TransportError>,
        ) -> Result<(), SendError> {
            if self.is_cancelled() {
                return Err(SendError::PolicyDenied);
            }
            write().map_err(SendError::Transport)
        }
    }

    struct ScriptedExchange<'a> {
        host: &'a str,
        path: &'a str,
        writes: RefCell<Vec<Vec<u8>>>,
        before: Option<&'a dyn Fn()>,
        after: Option<&'a dyn Fn()>,
        response: Result<Vec<u8>, SendError>,
    }
    impl Default for ScriptedExchange<'_> {
        fn default() -> Self {
            Self { host: "example.test", path: "/proxy/v1/chat/completions", writes: RefCell::new(Vec::new()), before: None, after: None,
                response: Ok(serde_json::to_vec(&serde_json::json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":r#"{"candidates":["日本語"]}"#}}]})).unwrap()) }
        }
    }
    impl Exchange for ScriptedExchange<'_> {
        fn exchange(
            &self,
            destination: &Destination,
            credential: &str,
            body: Vec<u8>,
            _: Instant,
            gate: &mut dyn SendGate,
        ) -> Result<Vec<u8>, SendError> {
            assert_eq!(destination.host(), self.host);
            assert_eq!(destination.base_path(), self.path);
            assert_eq!(credential, "synthetic-key");
            if let Some(before) = self.before {
                before();
            }
            let mut body = Some(body);
            gate.write_authorized(&mut || {
                self.writes.borrow_mut().push(body.take().unwrap());
                Ok(())
            })?;
            // Mutation must happen after the real authority send guard drops.
            // It must not deadlock, and its older result must not be accepted.
            if let Some(after) = self.after {
                after();
            }
            self.response.clone()
        }
    }

    fn ready() -> (PolicyAuthority, Destination) {
        let destination =
            Destination::for_chat_completions(Scheme::Https, "example.test", 443, "/proxy")
                .unwrap();
        let authority = PolicyAuthority::new(destination.clone());
        authority.set_model("fixture-model").unwrap();
        authority
            .replace_credential(Credential::new("synthetic-key".into()))
            .unwrap();
        authority.grant_manual_consent(&destination).unwrap();
        (authority, destination)
    }

    #[test]
    fn gemini_initial_and_additional_use_compatible_route_model_and_response() {
        let destination = Destination::for_chat_completions(
            Scheme::Https,
            "generativelanguage.googleapis.com",
            443,
            "/v1beta/openai/",
        )
        .unwrap();
        let authority = PolicyAuthority::new(destination.clone());
        authority.set_model("gemini-fixture-model").unwrap();
        authority
            .replace_credential(Credential::new("synthetic-key".into()))
            .unwrap();
        authority.grant_manual_consent(&destination).unwrap();
        let exchange = ScriptedExchange {
            host: "generativelanguage.googleapis.com",
            path: "/v1beta/openai/chat/completions",
            ..Default::default()
        };
        assert_eq!(
            convert_fixture(
                &authority,
                &destination,
                revision(&authority),
                &Admission::default(),
                &exchange
            )
            .unwrap(),
            "日本語"
        );
        let candidates = convert_additional_with_exchange(
            ManualRequest {
                revision: revision(&authority),
                destination: &destination,
                source: "nihongo",
                deadline: Instant::now() + Duration::from_secs(2),
            },
            "にほんご",
            &authority,
            &Admission::default(),
            &exchange,
        )
        .unwrap();
        assert!(!candidates.is_empty());
        let writes = exchange.writes.borrow();
        assert_eq!(writes.len(), 2);
        for body in writes.iter() {
            let body: serde_json::Value = serde_json::from_slice(body).unwrap();
            assert_eq!(body["model"], "gemini-fixture-model");
            assert_eq!(body["messages"][0]["role"], "system");
            assert_eq!(body["messages"][1]["role"], "user");
            assert!(body.get("api_key").is_none());
        }
    }

    fn revision(authority: &PolicyAuthority) -> Revision {
        let snapshot = authority.snapshot().unwrap();
        Revision {
            input: 1,
            settings: snapshot.settings,
            authorization: snapshot.authorization,
        }
    }

    fn convert_fixture(
        authority: &PolicyAuthority,
        destination: &Destination,
        revision: Revision,
        admission: &Admission,
        exchange: &ScriptedExchange<'_>,
    ) -> Result<String, SendError> {
        convert_with_exchange(
            ManualRequest {
                revision,
                destination,
                source: "nihongo",
                deadline: Instant::now() + Duration::from_secs(2),
            },
            authority,
            admission,
            exchange,
        )
    }

    #[test]
    fn native_chat_policy_selects_model_and_extracts_one_complete_candidate() {
        let (authority, destination) = ready();
        let exchange = ScriptedExchange::default();
        assert_eq!(
            convert_fixture(
                &authority,
                &destination,
                revision(&authority),
                &Admission::default(),
                &exchange
            ),
            Ok("日本語".into())
        );
        let writes = exchange.writes.borrow();
        assert_eq!(writes.len(), 1);
        assert_eq!(
            writes[0],
            crate::first_conversion::request_body("fixture-model", "nihongo").unwrap()
        );
    }

    #[test]
    fn native_chat_rejects_stale_missing_and_unconsented_policy_before_writing() {
        for change in 0..5 {
            let (authority, destination) = ready();
            let revision = revision(&authority);
            let admission = Admission::default();
            match change {
                0 => {
                    authority.set_model("new-model").unwrap();
                }
                1 => {
                    authority.revoke_manual_consent().unwrap();
                }
                2 => {
                    authority.replace_credential(None).unwrap();
                }
                3 => admission.0.set(true),
                _ => {
                    authority
                        .set_destination(
                            Destination::new(Scheme::Https, "other.test", 443, "/").unwrap(),
                        )
                        .unwrap();
                }
            }
            let exchange = ScriptedExchange::default();
            assert_eq!(
                convert_fixture(&authority, &destination, revision, &admission, &exchange),
                Err(SendError::PolicyDenied)
            );
            assert!(exchange.writes.borrow().is_empty());
        }
        let (authority, destination) = ready();
        let http = Destination::new(Scheme::HttpLoopback, "127.0.0.1", 1, "/").unwrap();
        let exchange = ScriptedExchange::default();
        assert_eq!(
            convert_fixture(
                &authority,
                &http,
                revision(&authority),
                &Admission::default(),
                &exchange
            ),
            Err(SendError::InvalidRequest)
        );
        let no_model = PolicyAuthority::new(destination.clone());
        no_model
            .replace_credential(Credential::new("synthetic-key".into()))
            .unwrap();
        no_model.grant_manual_consent(&destination).unwrap();
        assert_eq!(
            convert_fixture(
                &no_model,
                &destination,
                revision(&no_model),
                &Admission::default(),
                &exchange
            ),
            Err(SendError::PolicyDenied)
        );
        assert!(exchange.writes.borrow().is_empty());
    }

    #[test]
    fn native_chat_rechecks_the_actual_send_and_discards_changed_or_cancelled_results() {
        for after_send in [false, true] {
            for change in 0..4 {
                let (authority, destination) = ready();
                let admission = Admission::default();
                let mutate = || match change {
                    0 => {
                        authority.set_model("new-model").unwrap();
                    }
                    1 => {
                        authority.revoke_manual_consent().unwrap();
                    }
                    2 => {
                        authority
                            .replace_credential(Credential::new("replacement-key".into()))
                            .unwrap();
                    }
                    _ => admission.0.set(true),
                };
                let exchange = ScriptedExchange {
                    before: (!after_send).then_some(&mutate as &dyn Fn()),
                    after: after_send.then_some(&mutate as &dyn Fn()),
                    ..Default::default()
                };
                assert_eq!(
                    convert_fixture(
                        &authority,
                        &destination,
                        revision(&authority),
                        &admission,
                        &exchange
                    ),
                    Err(SendError::PolicyDenied)
                );
                assert_eq!(exchange.writes.borrow().len(), usize::from(after_send));
            }
        }
    }

    #[test]
    fn native_chat_rejects_invalid_candidates_and_preserves_fixed_transport_errors() {
        let (authority, destination) = ready();
        for (response, expected) in [
            (
                Ok(br#"{"choices":[]}"#.to_vec()),
                SendError::InvalidResponse,
            ),
            (
                Err(SendError::Transport(TransportError::Timeout)),
                SendError::Transport(TransportError::Timeout),
            ),
            (
                Err(SendError::Transport(TransportError::Connection)),
                SendError::Transport(TransportError::Connection),
            ),
        ] {
            let exchange = ScriptedExchange {
                response,
                ..Default::default()
            };
            assert_eq!(
                convert_fixture(
                    &authority,
                    &destination,
                    revision(&authority),
                    &Admission::default(),
                    &exchange
                ),
                Err(expected)
            );
            assert_eq!(exchange.writes.borrow().len(), 1);
        }
    }

    #[test]
    fn native_decisions_requires_separate_consent_and_cost_protection() {
        let (authority, destination) = ready();
        let budget = BudgetManager::new_in_memory(2);
        let states = vec![DecisionState::new(42, "nihongo", 1000)];
        let admission = Admission::default();
        let deadline = Instant::now() + Duration::from_secs(2);

        let decisions_json = serde_json::json!({
            "choices": [{
                "finish_reason": "stop",
                "message": {
                    "role": "assistant",
                    "content": r#"{"decisions":[{"id":42,"should_convert":true,"confidence":0.99,"converted_text":"日本語"}]}"#
                }
            }]
        });
        let exchange = ScriptedExchange {
            response: Ok(serde_json::to_vec(&decisions_json).unwrap()),
            ..Default::default()
        };

        // 1. Only manual consent is granted, auto consent is NOT granted
        let rev = revision(&authority);
        let err = evaluate_decisions_with_exchange(
            &destination,
            rev,
            &states,
            &authority,
            &admission,
            &budget,
            deadline,
            &exchange,
        );
        assert_eq!(err, Err(SendError::PolicyDenied));
        assert_eq!(budget.consumed(), 0);
        assert!(exchange.writes.borrow().is_empty());

        // 2. Grant auto consent
        authority.grant_auto_consent(&destination).unwrap();
        let rev = revision(&authority);
        let items = evaluate_decisions_with_exchange(
            &destination,
            rev,
            &states,
            &authority,
            &admission,
            &budget,
            deadline,
            &exchange,
        )
        .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, 42);
        assert!(items[0].should_convert);
        assert_eq!(items[0].converted_text.as_deref(), Some("日本語"));
        assert_eq!(budget.consumed(), 1);
        assert_eq!(exchange.writes.borrow().len(), 1);

        // 3. Exhaust budget (limit is 2)
        budget.reserve().unwrap();
        assert_eq!(budget.consumed(), 2);

        // 4. Over-budget fails with PolicyDenied without dispatching to exchange
        let err = evaluate_decisions_with_exchange(
            &destination,
            rev,
            &states,
            &authority,
            &admission,
            &budget,
            deadline,
            &exchange,
        );
        assert_eq!(err, Err(SendError::PolicyDenied));
        assert_eq!(budget.consumed(), 2);
        assert_eq!(exchange.writes.borrow().len(), 1); // no new write
    }

    #[test]
    #[ignore = "opt-in Internet test: posts a fixed initial-conversion fixture and dummy key to Postman Echo"]
    fn public_https_chat_path_rejects_echo_json_after_successful_native_io() {
        struct ObservedNative<'a> {
            native: NativeExchange<'a>,
            received: Cell<bool>,
        }
        impl Exchange for ObservedNative<'_> {
            fn exchange(
                &self,
                destination: &Destination,
                credential: &str,
                body: Vec<u8>,
                deadline: Instant,
                gate: &mut dyn SendGate,
            ) -> Result<Vec<u8>, SendError> {
                let bytes = self
                    .native
                    .exchange(destination, credential, body, deadline, gate)?;
                let echoed: serde_json::Value =
                    serde_json::from_slice(&bytes).expect("echo response was not JSON");
                assert_eq!(echoed["json"]["model"], "fixture-model");
                assert!(echoed["json"]["messages"][1]["content"]
                    .as_str()
                    .unwrap()
                    .ends_with("nihongo"));
                self.received.set(true);
                Ok(bytes)
            }
        }
        // Fixed synthetic data only. This test does not read settings or the
        // credential store, and is not a real conversion-service request.
        let destination =
            Destination::new(Scheme::Https, "postman-echo.com", 443, "/post").unwrap();
        let authority = PolicyAuthority::new(destination.clone());
        authority.set_model("fixture-model").unwrap();
        authority
            .replace_credential(Credential::new("sumibi-synthetic-test-key".into()))
            .unwrap();
        authority.grant_manual_consent(&destination).unwrap();
        let admission = Admission::default();
        let exchange = ObservedNative {
            native: NativeExchange {
                admission: &admission,
            },
            received: Cell::new(false),
        };
        let result = convert_with_exchange(
            ManualRequest {
                revision: revision(&authority),
                destination: &destination,
                source: "nihongo",
                deadline: Instant::now() + Duration::from_secs(20),
            },
            &authority,
            &admission,
            &exchange,
        );
        assert!(
            exchange.received.get(),
            "native HTTPS exchange did not complete"
        );
        assert_eq!(result, Err(SendError::InvalidResponse));
        println!("Host ordered send and native HTTPS passed; non-conversion echo JSON rejected");
    }

    #[test]
    fn native_preparation_refuses_diagnostic_http() {
        let destination =
            Destination::new(Scheme::HttpLoopback, "127.0.0.1", 8080, "/convert").unwrap();
        assert!(matches!(
            prepare(&destination),
            Err(Error::InvalidDestination)
        ));
    }

    #[test]
    fn receiving_requires_a_completed_send_and_denies_cancelled_results() {
        struct Gate(bool);
        impl SendGate for Gate {
            fn is_cancelled(&self) -> bool {
                self.0
            }
            fn write_authorized(
                &mut self,
                _: &mut dyn FnMut() -> Result<(), TransportError>,
            ) -> Result<(), SendError> {
                panic!("receive must not acquire the outbound send guard")
            }
        }
        let destination = Destination::new(Scheme::Https, "127.0.0.1", 1, "/convert").unwrap();
        assert_eq!(
            receive_body(prepare(&destination).unwrap(), &Gate(false), &|| false),
            Err(SendError::Transport(TransportError::InvalidRequest))
        );
        assert_eq!(
            receive_body(prepare(&destination).unwrap(), &Gate(true), &|| false),
            Err(SendError::PolicyDenied)
        );
    }

    #[test]
    fn denied_or_unused_send_gate_cannot_start_an_unguarded_request() {
        struct Gate(bool);
        impl SendGate for Gate {
            fn write_authorized(
                &mut self,
                _: &mut dyn FnMut() -> Result<(), TransportError>,
            ) -> Result<(), SendError> {
                if self.0 {
                    Ok(())
                } else {
                    Err(SendError::PolicyDenied)
                }
            }
        }
        let destination = Destination::new(Scheme::Https, "127.0.0.1", 1, "/convert").unwrap();
        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        assert!(matches!(
            send_body(
                &destination,
                "synthetic-key",
                vec![1],
                deadline,
                &mut Gate(false),
                &|| false
            ),
            Err(SendError::PolicyDenied)
        ));
        assert!(matches!(
            send_body(
                &destination,
                "synthetic-key",
                vec![1],
                deadline,
                &mut Gate(true),
                &|| false
            ),
            Err(SendError::InvalidResponse)
        ));
    }

    #[test]
    fn native_chat_additional_candidates_integration_and_fail_closed() {
        use sumibi_core::candidate::CandidateSource;

        let (authority, destination) = ready();
        let admission = Admission::default();

        // 1. Success integration
        let valid_json = serde_json::json!({
            "choices": [{
                "finish_reason": "stop",
                "message": {
                    "role": "assistant",
                    "content": r#"{"candidates":["炭火","澄火","墨美","こうしん"]}"#
                }
            }]
        });
        let exchange = ScriptedExchange {
            response: Ok(serde_json::to_vec(&valid_json).unwrap()),
            ..Default::default()
        };
        let request = ManualRequest {
            revision: revision(&authority),
            destination: &destination,
            source: "koushin",
            deadline: Instant::now() + Duration::from_secs(2),
        };
        let candidates =
            convert_additional_with_exchange(request, "更新", &authority, &admission, &exchange)
                .expect("additional conversion must succeed");

        // Index 0 must be current result
        assert_eq!(candidates[0].text, "更新");
        assert_eq!(candidates[0].source, CandidateSource::CurrentResult);

        // Next items should contain LLM candidates
        assert!(candidates
            .iter()
            .any(|c| c.text == "澄火" && c.source == CandidateSource::Llm));
        assert!(candidates
            .iter()
            .any(|c| c.text == "墨美" && c.source == CandidateSource::Llm));

        // And Sudachi homophone items should be present
        assert!(candidates
            .iter()
            .any(|c| c.source == CandidateSource::SudachiHomophone));

        // 2. Fail-closed: remote LLM failure must NOT perform offline substitution
        let failure_exchange = ScriptedExchange {
            response: Err(SendError::Transport(TransportError::Connection)),
            ..Default::default()
        };
        let req_fail = ManualRequest {
            revision: revision(&authority),
            destination: &destination,
            source: "sumibi",
            deadline: Instant::now() + Duration::from_secs(2),
        };
        let fail_result = convert_additional_with_exchange(
            req_fail,
            "炭火",
            &authority,
            &admission,
            &failure_exchange,
        );
        assert_eq!(
            fail_result,
            Err(SendError::Transport(TransportError::Connection))
        );
    }
}
