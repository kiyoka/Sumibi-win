//! Bounded Chat Completions payload for an initial manual conversion.
//! Pure serialization only: no HTTP, credentials, consent or execution.
use serde::Deserialize;
use serde_json::json;
use unicode_segmentation::UnicodeSegmentation;

const MAX_INPUT_GRAPHEMES: usize = 1000;
const MAX_RESPONSE_BYTES: usize = 64 * 1024;
const SYSTEM_PROMPT: &str = "あなたはローマ字と英語を、通常は自然な日本語へ変換するIMEです。ユーザーによる追加の変換指示で出力言語が指定された場合は、その言語へ翻訳してください。\nMarkdown記法、URL、固有名詞は可能な限り維持してください。\n入力にない情報は追加しないでください。\nユーザー辞書は登録されていません。\nユーザーによる追加の変換指示はありません。\nユーザーによる追加の変換指示がある場合はそれを反映し、ない場合は文脈に最も自然な日本語変換を1件だけ作ってください。{\"candidates\":[\"候補1\"]}というJSONだけを返してください。\nJSON以外の説明やMarkdownのコードフェンスは返さないでください。";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayloadError {
    InvalidModel,
    InvalidSource,
    InvalidResponse,
}

pub(crate) fn valid_model(model: &str) -> bool {
    !model.is_empty() && model.len() <= 256 && !model.chars().any(char::is_control)
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}
#[derive(Deserialize)]
struct Choice {
    finish_reason: Option<String>,
    message: Message,
}
#[derive(Deserialize)]
struct Message {
    role: String,
    content: Option<String>,
    refusal: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Candidates {
    candidates: Vec<String>,
}

/// Create only the initial-conversion body. Model is explicit; no preset
/// options, credential, endpoint or destination is inferred here.
pub fn request_body(model: &str, source: &str) -> Result<Vec<u8>, PayloadError> {
    if !valid_model(model) {
        return Err(PayloadError::InvalidModel);
    }
    if !sumibi_protocol::message::valid_text(source)
        || source.graphemes(true).take(MAX_INPUT_GRAPHEMES + 1).count() > MAX_INPUT_GRAPHEMES
    {
        return Err(PayloadError::InvalidSource);
    }
    let user = format!("周辺文脈：\n\n\n現在の変換結果：\nなし\n\n変換対象：\n{source}");
    serde_json::to_vec(&json!({
        "model": model,
        "messages": [
            {"role": "system", "content": SYSTEM_PROMPT},
            {"role": "user", "content": user}
        ]
    }))
    .map_err(|_| PayloadError::InvalidSource)
}

/// Extract one candidate only from a complete, non-refused response. We do
/// not treat arbitrary prose or a partial JSON fragment as a conversion.
pub fn response_candidate(bytes: &[u8]) -> Result<String, PayloadError> {
    if bytes.is_empty() || bytes.len() > MAX_RESPONSE_BYTES {
        return Err(PayloadError::InvalidResponse);
    }
    let response: ChatResponse =
        serde_json::from_slice(bytes).map_err(|_| PayloadError::InvalidResponse)?;
    let choices = &response.choices;
    if choices.len() != 1 {
        return Err(PayloadError::InvalidResponse);
    }
    let choice = &choices[0];
    if choice.finish_reason.as_deref() != Some("stop") {
        return Err(PayloadError::InvalidResponse);
    }
    let message = &choice.message;
    if message.role != "assistant" || message.refusal.is_some() {
        return Err(PayloadError::InvalidResponse);
    }
    let content = message
        .content
        .as_deref()
        .ok_or(PayloadError::InvalidResponse)?;
    let payload: Candidates =
        serde_json::from_str(content).map_err(|_| PayloadError::InvalidResponse)?;
    if payload.candidates.len() != 1 {
        return Err(PayloadError::InvalidResponse);
    }
    let candidate = payload.candidates[0].trim();
    if !sumibi_protocol::message::valid_text(candidate) {
        return Err(PayloadError::InvalidResponse);
    }
    Ok(candidate.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn reply(content: &str, finish: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({"choices":[{"finish_reason":finish,"message":{"role":"assistant","content":content}}]})).unwrap()
    }

    #[test]
    fn request_uses_explicit_model_and_mac_initial_prompt_without_secret() {
        let body: Value =
            serde_json::from_slice(&request_body("fixture-model", "nihongo").unwrap()).unwrap();
        assert_eq!(body["model"], "fixture-model");
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][0]["content"], SYSTEM_PROMPT);
        assert_eq!(
            body["messages"][1]["content"],
            "周辺文脈：\n\n\n現在の変換結果：\nなし\n\n変換対象：\nnihongo"
        );
        assert_eq!(body.as_object().unwrap().len(), 2);
        assert_eq!(request_body("", "x"), Err(PayloadError::InvalidModel));
        assert_eq!(request_body("a\nb", "x"), Err(PayloadError::InvalidModel));
        assert_eq!(request_body("m", ""), Err(PayloadError::InvalidSource));
        assert_eq!(
            request_body("m", &"a".repeat(1001)),
            Err(PayloadError::InvalidSource)
        );
        assert!(request_body("m", &"e\u{301}".repeat(500)).is_ok());
    }

    #[test]
    fn response_accepts_only_one_complete_safe_candidate() {
        assert_eq!(
            response_candidate(&reply(r#"{"candidates":[" 日本語 "]}"#, "stop")),
            Ok("日本語".into())
        );
        for content in [
            "日本語",
            r#"{"candidates":[]}"#,
            r#"{"candidates":["a","b"]}"#,
            r#"{"candidates":["" ]}"#,
            r#"{"candidates":["a"],"extra":1}"#,
            r#"{"candidates":["a"],"candidates":["b"]}"#,
        ] {
            assert_eq!(
                response_candidate(&reply(content, "stop")),
                Err(PayloadError::InvalidResponse)
            );
        }
        assert_eq!(
            response_candidate(&reply(r#"{"candidates":["a"]}"#, "length")),
            Err(PayloadError::InvalidResponse)
        );
        let refused = json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":r#"{"candidates":["a"]}"#,"refusal":"no"}}]});
        assert_eq!(
            response_candidate(&serde_json::to_vec(&refused).unwrap()),
            Err(PayloadError::InvalidResponse)
        );
        assert_eq!(
            response_candidate(&vec![b' '; MAX_RESPONSE_BYTES + 1]),
            Err(PayloadError::InvalidResponse)
        );
    }
}
