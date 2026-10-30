//! Fail-closed policy checks around one worker-only conversion transport call.
//! Snapshot acquisition must be coherent; the host will later supply it from
//! its settings/credential authority. No implicit retry or destination fallback.
use crate::policy::PolicyAuthority;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use sumibi_protocol::message::{self, Revision};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    Https,
    HttpLoopback,
}

/// Exact, canonical identity used for both transport routing and consent.
/// The restricted constructor rejects ambiguous path/host spellings.
#[derive(Clone, PartialEq, Eq)]
pub struct Destination {
    scheme: Scheme,
    host: String,
    port: u16,
    base_path: String,
}
impl Destination {
    pub fn new(scheme: Scheme, host: &str, port: u16, base_path: &str) -> Option<Self> {
        if port == 0 || !valid_path(base_path) {
            return None;
        }
        let host = if let Ok(ip) = host.parse::<IpAddr>() {
            if scheme == Scheme::HttpLoopback && !ip.is_loopback() {
                return None;
            }
            ip.to_string()
        } else {
            if !valid_dns_host(host)
                || (scheme == Scheme::HttpLoopback && !host.eq_ignore_ascii_case("localhost"))
            {
                return None;
            }
            host.to_ascii_lowercase()
        };
        Some(Self {
            scheme,
            host,
            port,
            base_path: base_path.to_owned(),
        })
    }

    pub fn scheme(&self) -> Scheme {
        self.scheme
    }
    pub fn host(&self) -> &str {
        &self.host
    }
    pub fn port(&self) -> u16 {
        self.port
    }
    pub fn base_path(&self) -> &str {
        &self.base_path
    }
}

fn valid_dns_host(host: &str) -> bool {
    host.len() <= 253
        && !host.is_empty()
        && host.split('.').all(|label| {
            (1..=63).contains(&label.len())
                && label
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && label
                    .as_bytes()
                    .last()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

fn valid_path(path: &str) -> bool {
    path.starts_with('/')
        && !path.contains("//")
        && path.split('/').skip(1).all(|segment| {
            segment != "."
                && segment != ".."
                && segment.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~')
                })
        })
}

/// Secret bytes remain in the Host; this type intentionally has no Debug.
pub struct Credential(String);
impl Credential {
    pub fn new(value: String) -> Option<Self> {
        (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_graphic()))
            .then_some(Self(value))
    }
    fn expose(&self) -> &str {
        &self.0
    }
}

/// Snapshot of the Host's settings and credential authority. On any policy
/// change, the provider must publish a new coherent snapshot and increment its
/// corresponding revision before acknowledging the change to the user.
#[derive(Clone)]
pub struct PolicySnapshot {
    pub settings: u64,
    pub authorization: u64,
    pub model: Option<String>,
    pub destination: Destination,
    pub manual_consent: Option<Destination>,
    pub credential: Option<Arc<Credential>>,
    pub allowed: bool,
}

pub trait PolicySource {
    fn snapshot(&self) -> Option<PolicySnapshot>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportError {
    InvalidRequest,
    Connection,
    Timeout,
    Http,
    Redirect,
    Authentication,
    RateLimited,
    ResponseTooLarge,
    InvalidResponse,
    Cancelled,
}

/// Snapshot-only transport used by policy unit tests. Shipping adapters use
/// `OrderedConversionTransport` so revocation orders the outbound bytes.
#[cfg(test)]
pub trait ConversionTransport {
    fn convert(
        &self,
        destination: &Destination,
        credential: &str,
        source: &str,
        deadline: Instant,
    ) -> Result<String, TransportError>;
}

/// A trusted adapter must place every outbound request byte, including the
/// credential header, inside this callback. It may connect before calling it.
pub trait SendGate {
    fn is_cancelled(&self) -> bool {
        false
    }
    fn write_authorized(
        &mut self,
        write: &mut dyn FnMut() -> Result<(), TransportError>,
    ) -> Result<(), SendError>;
}

/// Connection-local cancellation must be ordered with the outbound write,
/// independently of Host policy revocation.
pub trait SendAdmission {
    fn is_cancelled(&self) -> bool;
    fn request_deadline(&self) -> Option<Instant> {
        None
    }
    fn write_if_active(
        &self,
        write: &mut dyn FnMut() -> Result<(), TransportError>,
    ) -> Result<(), SendError>;
}

pub trait OrderedConversionTransport {
    fn convert_ordered(
        &self,
        destination: &Destination,
        credential: &str,
        source: &str,
        deadline: Instant,
        gate: &mut dyn SendGate,
    ) -> Result<String, SendError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendError {
    InvalidRequest,
    PolicyDenied,
    Timeout,
    InvalidResponse,
    Transport(TransportError),
}

impl From<TransportError> for SendError {
    fn from(error: TransportError) -> Self {
        Self::Transport(error)
    }
}

pub struct ManualRequest<'a> {
    pub revision: Revision,
    pub destination: &'a Destination,
    pub source: &'a str,
    pub deadline: Instant,
}

/// Snapshot-only policy model for unit tests. A real outbound adapter must
/// call `convert_ordered` after input protection and capacity reservation.
#[cfg(test)]
pub fn convert(
    request: ManualRequest<'_>,
    policy: &impl PolicySource,
    transport: &impl ConversionTransport,
) -> Result<String, SendError> {
    let remaining = request.deadline.saturating_duration_since(Instant::now());
    if request.revision.input == 0
        || request.revision.settings == 0
        || request.revision.authorization == 0
        || !message::valid_text(request.source)
        || remaining > Duration::from_secs(60)
    {
        return Err(SendError::InvalidRequest);
    }
    let before = policy.snapshot().ok_or(SendError::PolicyDenied)?;
    let credential = authorized(&before, &request)?;
    if Instant::now() >= request.deadline {
        return Err(SendError::Timeout);
    }
    let result = transport
        .convert(
            request.destination,
            credential.expose(),
            request.source,
            request.deadline,
        )
        .map_err(SendError::Transport)?;
    if Instant::now() >= request.deadline {
        return Err(SendError::Timeout);
    }
    let after = policy.snapshot().ok_or(SendError::PolicyDenied)?;
    let current_credential = authorized(&after, &request)?;
    if !Arc::ptr_eq(&credential, &current_credential) {
        return Err(SendError::PolicyDenied);
    }
    if !message::valid_text(&result) {
        return Err(SendError::InvalidResponse);
    }
    if Instant::now() >= request.deadline {
        return Err(SendError::Timeout);
    }
    Ok(result)
}

struct AuthoritySendGate<'a> {
    authority: &'a PolicyAuthority,
    revision: Revision,
    destination: &'a Destination,
    credential: &'a Arc<Credential>,
    admission: &'a dyn SendAdmission,
    deadline: Instant,
    used: bool,
    sent: bool,
}

impl SendGate for AuthoritySendGate<'_> {
    fn is_cancelled(&self) -> bool {
        self.admission.is_cancelled()
    }

    fn write_authorized(
        &mut self,
        write: &mut dyn FnMut() -> Result<(), TransportError>,
    ) -> Result<(), SendError> {
        if self.used {
            return Err(SendError::InvalidRequest);
        }
        self.used = true;
        if Instant::now() >= self.deadline {
            return Err(SendError::Timeout);
        }
        let _guard = self
            .authority
            .authorize_send_start(self.revision, self.destination, self.credential)
            .map_err(|_| SendError::PolicyDenied)?;
        if Instant::now() >= self.deadline {
            return Err(SendError::Timeout);
        }
        self.admission.write_if_active(write)?;
        self.sent = true;
        Ok(())
    }
}

/// Product-facing send boundary. The adapter must call `write_authorized`
/// exactly once around its outbound request write. Revocation that finishes
/// before the callback starts denies all request bytes; a concurrent revoke
/// waits for the bounded write to finish before acknowledging the new policy.
pub fn convert_ordered(
    request: ManualRequest<'_>,
    authority: &PolicyAuthority,
    admission: &impl SendAdmission,
    transport: &impl OrderedConversionTransport,
) -> Result<String, SendError> {
    if admission.is_cancelled() {
        return Err(SendError::PolicyDenied);
    }
    if admission
        .request_deadline()
        .is_some_and(|accepted| request.deadline > accepted)
    {
        return Err(SendError::InvalidRequest);
    }
    let remaining = request.deadline.saturating_duration_since(Instant::now());
    if request.revision.input == 0
        || request.revision.settings == 0
        || request.revision.authorization == 0
        || !message::valid_text(request.source)
        || remaining > Duration::from_secs(60)
    {
        return Err(SendError::InvalidRequest);
    }
    let before = authority.snapshot().ok_or(SendError::PolicyDenied)?;
    let credential = authorized(&before, &request)?;
    if Instant::now() >= request.deadline {
        return Err(SendError::Timeout);
    }
    let mut gate = AuthoritySendGate {
        authority,
        revision: request.revision,
        destination: request.destination,
        credential: &credential,
        admission,
        deadline: request.deadline,
        used: false,
        sent: false,
    };
    let result = transport.convert_ordered(
        request.destination,
        credential.expose(),
        request.source,
        request.deadline,
        &mut gate,
    )?;
    if !gate.sent {
        return Err(SendError::InvalidResponse);
    }
    if admission.is_cancelled() {
        return Err(SendError::PolicyDenied);
    }
    if Instant::now() >= request.deadline {
        return Err(SendError::Timeout);
    }
    let after = authority.snapshot().ok_or(SendError::PolicyDenied)?;
    let current_credential = authorized(&after, &request)?;
    if !Arc::ptr_eq(&credential, &current_credential) {
        return Err(SendError::PolicyDenied);
    }
    if !message::valid_text(&result) {
        return Err(SendError::InvalidResponse);
    }
    if admission.is_cancelled() {
        return Err(SendError::PolicyDenied);
    }
    if Instant::now() >= request.deadline {
        return Err(SendError::Timeout);
    }
    Ok(result)
}

fn authorized(
    snapshot: &PolicySnapshot,
    request: &ManualRequest<'_>,
) -> Result<Arc<Credential>, SendError> {
    if !snapshot.allowed
        || snapshot.settings != request.revision.settings
        || snapshot.authorization != request.revision.authorization
        || snapshot.destination != *request.destination
        || snapshot.manual_consent.as_ref() != Some(request.destination)
    {
        return Err(SendError::PolicyDenied);
    }
    snapshot.credential.clone().ok_or(SendError::PolicyDenied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::time::Duration;

    struct Active;
    impl SendAdmission for Active {
        fn is_cancelled(&self) -> bool {
            false
        }
        fn write_if_active(
            &self,
            write: &mut dyn FnMut() -> Result<(), TransportError>,
        ) -> Result<(), SendError> {
            write().map_err(SendError::Transport)
        }
    }

    struct Source(RefCell<PolicySnapshot>);
    impl PolicySource for Source {
        fn snapshot(&self) -> Option<PolicySnapshot> {
            Some(self.0.borrow().clone())
        }
    }

    struct Mock<'a> {
        calls: Cell<usize>,
        result: &'static str,
        during: Option<Box<dyn Fn() + 'a>>,
    }
    impl ConversionTransport for Mock<'_> {
        fn convert(
            &self,
            _: &Destination,
            credential: &str,
            source: &str,
            _: Instant,
        ) -> Result<String, TransportError> {
            assert_eq!(credential, "synthetic-key");
            assert_eq!(source, "nihongo");
            self.calls.set(self.calls.get() + 1);
            if let Some(during) = &self.during {
                during();
            }
            Ok(self.result.into())
        }
    }

    fn destination() -> Destination {
        Destination::new(Scheme::Https, "API.Example.test", 443, "/v1").unwrap()
    }
    fn source() -> Source {
        let destination = destination();
        Source(RefCell::new(PolicySnapshot {
            settings: 1,
            authorization: 1,
            model: None,
            destination: destination.clone(),
            manual_consent: Some(destination),
            credential: Some(Arc::new(Credential::new("synthetic-key".into()).unwrap())),
            allowed: true,
        }))
    }
    fn request(destination: &Destination) -> ManualRequest<'_> {
        ManualRequest {
            revision: Revision {
                input: 1,
                settings: 1,
                authorization: 1,
            },
            destination,
            source: "nihongo",
            deadline: Instant::now() + Duration::from_secs(1),
        }
    }
    fn mock() -> Mock<'static> {
        Mock {
            calls: Cell::new(0),
            result: "日本語",
            during: None,
        }
    }

    struct OrderedMock<'a> {
        authority: &'a PolicyAuthority,
        revoke_before: bool,
        revoke_after: bool,
        omit_gate: bool,
        bytes_written: Cell<usize>,
    }

    impl OrderedConversionTransport for OrderedMock<'_> {
        fn convert_ordered(
            &self,
            _: &Destination,
            _: &str,
            _: &str,
            _: Instant,
            gate: &mut dyn SendGate,
        ) -> Result<String, SendError> {
            if self.revoke_before {
                self.authority
                    .revoke_manual_consent()
                    .map_err(|_| SendError::PolicyDenied)?;
            }
            if !self.omit_gate {
                gate.write_authorized(&mut || {
                    self.bytes_written.set(self.bytes_written.get() + 1);
                    Ok(())
                })?;
            }
            if self.revoke_after {
                self.authority
                    .revoke_manual_consent()
                    .map_err(|_| SendError::PolicyDenied)?;
            }
            Ok("日本語".into())
        }
    }

    fn ordered_authority(destination: &Destination) -> PolicyAuthority {
        let authority = PolicyAuthority::new(destination.clone());
        authority
            .replace_credential(Some(Credential::new("synthetic-key".into()).unwrap()))
            .unwrap();
        authority.grant_manual_consent(destination).unwrap();
        authority
    }

    fn ordered_request<'a>(
        destination: &'a Destination,
        authority: &PolicyAuthority,
    ) -> ManualRequest<'a> {
        let policy = authority.wire_policy().unwrap();
        ManualRequest {
            revision: Revision {
                input: 1,
                settings: policy.settings,
                authorization: policy.authorization,
            },
            destination,
            source: "nihongo",
            deadline: Instant::now() + Duration::from_secs(1),
        }
    }

    #[test]
    fn ordered_send_denies_revocation_before_first_byte_and_later_result() {
        let destination = destination();
        let authority = ordered_authority(&destination);
        let transport = OrderedMock {
            authority: &authority,
            revoke_before: true,
            revoke_after: false,
            omit_gate: false,
            bytes_written: Cell::new(0),
        };
        assert_eq!(
            convert_ordered(
                ordered_request(&destination, &authority),
                &authority,
                &Active,
                &transport
            ),
            Err(SendError::PolicyDenied)
        );
        assert_eq!(transport.bytes_written.get(), 0);

        let authority = ordered_authority(&destination);
        let transport = OrderedMock {
            authority: &authority,
            revoke_before: false,
            revoke_after: true,
            omit_gate: false,
            bytes_written: Cell::new(0),
        };
        assert_eq!(
            convert_ordered(
                ordered_request(&destination, &authority),
                &authority,
                &Active,
                &transport
            ),
            Err(SendError::PolicyDenied)
        );
        assert_eq!(transport.bytes_written.get(), 1);
    }

    #[test]
    fn ordered_adapter_must_use_gate_before_success() {
        let destination = destination();
        let authority = ordered_authority(&destination);
        let transport = OrderedMock {
            authority: &authority,
            revoke_before: false,
            revoke_after: false,
            omit_gate: true,
            bytes_written: Cell::new(0),
        };
        assert_eq!(
            convert_ordered(
                ordered_request(&destination, &authority),
                &authority,
                &Active,
                &transport
            ),
            Err(SendError::InvalidResponse)
        );
        let transport = OrderedMock {
            omit_gate: false,
            ..transport
        };
        assert_eq!(
            convert_ordered(
                ordered_request(&destination, &authority),
                &authority,
                &Active,
                &transport
            ),
            Ok("日本語".into())
        );
        assert_eq!(transport.bytes_written.get(), 1);
    }

    #[test]
    fn destination_identity_is_canonical_and_http_is_loopback_only() {
        assert_eq!(destination().host(), "api.example.test");
        assert_eq!(destination().port(), 443);
        assert!(matches!(destination().scheme(), Scheme::Https));
        assert_eq!(destination().base_path(), "/v1");
        assert!(
            Destination::new(Scheme::HttpLoopback, "LOCALHOST", 8080, "/v1")
                == Destination::new(Scheme::HttpLoopback, "localhost", 8080, "/v1")
        );
        for host in ["example.test", "192.0.2.1", "0.0.0.0", "[::1]"] {
            assert!(Destination::new(Scheme::HttpLoopback, host, 8080, "/v1").is_none());
        }
        assert!(Destination::new(Scheme::HttpLoopback, "::1", 8080, "/v1").is_some());
        for path in [
            "v1",
            "/../v1",
            "/v1?x=1",
            "/v1%2fadmin",
            "/v1\\other",
            "/v1//x",
        ] {
            assert!(Destination::new(Scheme::Https, "example.test", 443, path).is_none());
        }
        assert!(Credential::new("key\nInjected: header".into()).is_none());
    }

    #[test]
    fn consent_revision_destination_and_key_gate_before_transport() {
        let source = source();
        let transport = mock();
        let destination = destination();
        assert_eq!(
            convert(request(&destination), &source, &transport),
            Ok("日本語".into())
        );
        assert_eq!(transport.calls.get(), 1);
        source.0.borrow_mut().manual_consent = None;
        assert_eq!(
            convert(request(&destination), &source, &transport),
            Err(SendError::PolicyDenied)
        );
        source.0.borrow_mut().manual_consent = Some(destination.clone());
        source.0.borrow_mut().authorization = 2;
        assert_eq!(
            convert(request(&destination), &source, &transport),
            Err(SendError::PolicyDenied)
        );
        source.0.borrow_mut().authorization = 1;
        source.0.borrow_mut().credential = None;
        assert_eq!(
            convert(request(&destination), &source, &transport),
            Err(SendError::PolicyDenied)
        );
        assert_eq!(transport.calls.get(), 1);
        let other = Destination::new(Scheme::Https, "example.test", 443, "/v1").unwrap();
        assert_eq!(
            convert(request(&other), &source, &transport),
            Err(SendError::PolicyDenied)
        );
    }

    #[test]
    fn credential_replacement_during_transport_discards_result() {
        let source = source();
        let destination = destination();
        let transport = Mock {
            calls: Cell::new(0),
            result: "日本語",
            during: Some(Box::new(|| {
                source.0.borrow_mut().credential =
                    Some(Arc::new(Credential::new("synthetic-key".into()).unwrap()));
            })),
        };
        assert_eq!(
            convert(request(&destination), &source, &transport),
            Err(SendError::PolicyDenied)
        );
        assert_eq!(transport.calls.get(), 1);
    }

    #[test]
    fn consent_revocation_during_transport_discards_result() {
        let source = source();
        let destination = destination();
        let transport = Mock {
            calls: Cell::new(0),
            result: "日本語",
            during: Some(Box::new(|| {
                let mut snapshot = source.0.borrow_mut();
                snapshot.authorization += 1;
                snapshot.manual_consent = None;
            })),
        };
        assert_eq!(
            convert(request(&destination), &source, &transport),
            Err(SendError::PolicyDenied)
        );
        assert_eq!(transport.calls.get(), 1);
    }

    #[test]
    fn invalid_input_deadline_and_response_do_not_escape() {
        let source = source();
        let destination = destination();
        let transport = mock();
        let mut invalid = request(&destination);
        invalid.source = "";
        assert_eq!(
            convert(invalid, &source, &transport),
            Err(SendError::InvalidRequest)
        );
        let mut expired = request(&destination);
        expired.deadline = Instant::now();
        assert_eq!(
            convert(expired, &source, &transport),
            Err(SendError::Timeout)
        );
        assert_eq!(transport.calls.get(), 0);
        let mut overlong = request(&destination);
        overlong.deadline = Instant::now() + Duration::from_secs(61);
        assert_eq!(
            convert(overlong, &source, &transport),
            Err(SendError::InvalidRequest)
        );
        assert_eq!(transport.calls.get(), 0);
        let bad_response = Mock {
            result: "",
            ..mock()
        };
        assert_eq!(
            convert(request(&destination), &source, &bad_response),
            Err(SendError::InvalidResponse)
        );
    }
}
