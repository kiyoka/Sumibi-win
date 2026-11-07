//! Provider presets only; the saved destination and editable model are authoritative.
use crate::manual::{Destination, Scheme};

pub const GEMINI_MODELS: [&str; 2] = ["gemini-4-argon", "gemini-3.6-flash"];
const DEFAULT_GEMINI_MODEL: &str = GEMINI_MODELS[1];

pub(crate) fn gemini_manual_options(
    destination: &Destination,
    model: &str,
) -> Option<(&'static str, Option<f64>)> {
    if Provider::from_destination(destination) != Provider::Google {
        return None;
    }
    match model {
        "gemini-4-argon" => Some(("low", None)),
        "gemini-3.6-flash" => Some(("minimal", Some(1.0))),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    OpenAi,
    Google,
    Other,
}

impl Provider {
    pub fn from_destination(destination: &Destination) -> Self {
        if destination.scheme() != Scheme::Https || destination.port() != 443 {
            return Self::Other;
        }
        match (destination.host(), destination.base_path()) {
            ("api.openai.com", "/v1/chat/completions") => Self::OpenAi,
            ("generativelanguage.googleapis.com", "/v1beta/openai/chat/completions") => {
                Self::Google
            }
            _ => Self::Other,
        }
    }

    pub fn preset(self) -> Option<(&'static str, &'static str, &'static str)> {
        match self {
            Self::OpenAi => Some(("api.openai.com", "/v1", "gpt-4o-mini")),
            Self::Google => Some((
                "generativelanguage.googleapis.com",
                "/v1beta/openai",
                DEFAULT_GEMINI_MODEL,
            )),
            Self::Other => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn presets_resolve_and_classify_without_changing_custom_endpoints() {
        for provider in [Provider::OpenAi, Provider::Google] {
            let (host, path, _) = provider.preset().unwrap();
            for path in [
                path.to_owned(),
                format!("{path}/"),
                format!("{path}/chat/completions"),
            ] {
                let destination =
                    Destination::for_chat_completions(Scheme::Https, host, 443, &path).unwrap();
                assert_eq!(Provider::from_destination(&destination), provider);
                assert!(!destination.base_path().contains("openai/v1/"));
            }
        }
        let custom =
            Destination::for_chat_completions(Scheme::Https, "api.openai.com", 443, "/proxy")
                .unwrap();
        assert_eq!(Provider::from_destination(&custom), Provider::Other);
        assert_eq!(custom.base_path(), "/proxy/v1/chat/completions");
    }
}
