//! Provider presets only; the saved destination and editable model are authoritative.
use crate::manual::{Destination, Scheme};

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
                "gemini-3.6-flash",
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
