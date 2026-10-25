//! Strict version-one messages. IDs and handshake echoes are not OS credentials.
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub const VERSION: u16 = 1;
/// Provisional single-result protocol cap, aligned with the diagnostic TSF host.
/// This is not the future product's grapheme limit.
pub const MAX_TEXT_UNITS: usize = 1024;
pub const MAX_TIMEOUT_MS: u32 = 60_000;

/// A caller-generated, nonzero lifetime identity. Production callers must use
/// fresh unpredictable values, never addresses, PIDs or document identifiers.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Id(u128);

impl Id {
    pub fn new(value: u128) -> Option<Self> {
        (value != 0).then_some(Self(value))
    }
}

impl Serialize for Id {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!("{:032x}", self.0))
    }
}

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        if text.len() != 32
            || !text
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(serde::de::Error::custom("invalid identity"));
        }
        u128::from_str_radix(&text, 16)
            .ok()
            .and_then(Self::new)
            .ok_or_else(|| serde::de::Error::custom("invalid identity"))
    }
}

// Text-bearing and identity-bearing types deliberately omit Debug. Parser
// diagnostics are discarded rather than exposing unknown fields or values.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub version: u16,
    pub session: Id,
    pub generation: u64,
    pub message: Message,
}

/// Externally tagged: exactly one message key, with a strict payload structure.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Message {
    Hello(Hello),
    Ready(Ready),
    Convert(Convert),
    Cancel(Cancel),
    Result(ConversionResult),
    Error(ConversionError),
    PolicyChanged(Policy),
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hello {
    pub broker: Id,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ready {
    pub broker: Id,
    pub policy: Policy,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub settings: u64,
    pub authorization: u64,
    pub allowed: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Revision {
    pub input: u64,
    pub settings: u64,
    pub authorization: u64,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Convert {
    pub request: u64,
    pub revision: Revision,
    pub timeout_ms: u32,
    pub text: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cancel {
    pub request: u64,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionResult {
    pub request: u64,
    pub revision: Revision,
    pub text: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionError {
    pub request: u64,
    pub revision: Revision,
    pub code: ErrorCode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Cancelled,
    Timeout,
    Unavailable,
    Rejected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageError {
    TooLarge,
    Malformed,
    UnsupportedVersion,
    InvalidValue,
}

impl Envelope {
    pub fn validate(&self) -> Result<(), MessageError> {
        if self.version != VERSION {
            return Err(MessageError::UnsupportedVersion);
        }
        let valid = self.generation != 0
            && match &self.message {
                Message::Hello(_) => true,
                Message::Ready(ready) => ready.policy.valid(),
                Message::Convert(convert) => {
                    convert.request != 0
                        && convert.revision.valid()
                        && (1..=MAX_TIMEOUT_MS).contains(&convert.timeout_ms)
                        && valid_text(&convert.text)
                }
                Message::Cancel(cancel) => cancel.request != 0,
                Message::Result(result) => {
                    result.request != 0 && result.revision.valid() && valid_text(&result.text)
                }
                Message::Error(error) => error.request != 0 && error.revision.valid(),
                Message::PolicyChanged(policy) => policy.valid(),
            };
        if valid {
            Ok(())
        } else {
            Err(MessageError::InvalidValue)
        }
    }
}

impl Policy {
    /// A policy epoch is present only when both revision counters are nonzero.
    pub fn valid(self) -> bool {
        self.settings != 0 && self.authorization != 0
    }

    /// Both epochs are monotonic; changing permission advances the authority
    /// epoch even when the final value returns to its earlier state.
    pub fn follows(self, previous: Self) -> bool {
        self.valid()
            && previous.valid()
            && self.settings >= previous.settings
            && self.authorization >= previous.authorization
            && (self.allowed == previous.allowed || self.authorization > previous.authorization)
    }
}

impl Revision {
    fn valid(self) -> bool {
        self.input != 0 && self.settings != 0 && self.authorization != 0
    }
}

/// Shared, allocation-free single-result text limit for transport adapters.
pub fn valid_text(text: &str) -> bool {
    !text.is_empty()
        && !text.contains('\u{fffc}')
        && text.len() <= MAX_TEXT_UNITS * 4
        && text.encode_utf16().take(MAX_TEXT_UNITS + 1).count() <= MAX_TEXT_UNITS
}

/// Decode one complete UTF-8 frame body. All diagnostics are fixed enums.
pub fn decode(body: &str) -> Result<Envelope, MessageError> {
    if body.len() > crate::MAX_FRAME_BYTES {
        return Err(MessageError::TooLarge);
    }
    let envelope: Envelope = serde_json::from_str(body).map_err(|_| MessageError::Malformed)?;
    envelope.validate()?;
    Ok(envelope)
}

/// Validate before serialization, then include the existing length prefix.
pub fn encode(envelope: &Envelope) -> Result<Vec<u8>, MessageError> {
    envelope.validate()?;
    let body = serde_json::to_string(envelope).map_err(|_| MessageError::Malformed)?;
    crate::encode(&body).map_err(|_| MessageError::TooLarge)
}
