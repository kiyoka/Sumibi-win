//! Sumibi Decisions: auto-conversion evaluation, cost protection and response verification.
//! Implements ADR 0102: separate consent, 200ms fixed window, strict validation, and 10,000 request budget cap.

use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
pub use sumibi_core::decisions::{
    validate_decision_response, DecisionError, DecisionResponse, DecisionResultItem, DecisionState,
    CONFIDENCE_THRESHOLD, MAX_DECISIONS_BUDGET, MAX_GRAPHEMES_PER_STATE, MAX_PENDING_STATES,
    MAX_STATES_PER_BATCH, WINDOW_DURATION_MS,
};
use unicode_segmentation::UnicodeSegmentation;

const DECISIONS_SYSTEM_PROMPT: &str = "あなたは入力中のテキストを監視し、自然な日本語へ自動変換すべき確定的な状態かどうかを判定するIME判定エンジンです。\n各状態（id, text）について、以下のJSON形式のみを返してください：\n{\"decisions\":[{\"id\":<id>,\"should_convert\":<true|false>,\"confidence\":<0.0〜1.0>,\"converted_text\":<変換後の日本語またはnull>}]}\nJSON以外の説明やMarkdownのコードフェンスは返さないでください。";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecisionsError {
    InvalidModel,
    EmptyStates,
    TooManyStates,
    GraphemesExceeded,
    BudgetExhausted,
    PersistenceFailed,
    InvalidResponse,
    PolicyDenied,
}

#[derive(Serialize, Deserialize)]
struct StoredBudget {
    version: u32,
    consumed_count: u64,
    limit: u64,
}

/// Thread-safe budget manager enforcing persistent request pre-allocation (ADR 0102).
/// Fails closed: if persistence fails, no network request is dispatched.
pub struct BudgetManager {
    file_path: PathBuf,
    consumed: AtomicU64,
    limit: u64,
    lock: Mutex<()>,
}

impl BudgetManager {
    pub fn new(file_path: PathBuf, limit: u64) -> Result<Self, DecisionsError> {
        let mut consumed = 0;
        if file_path.exists() {
            if let Ok(mut file) = fs::File::open(&file_path) {
                let mut content = String::new();
                if file.read_to_string(&mut content).is_ok() {
                    if let Ok(record) = serde_json::from_str::<StoredBudget>(&content) {
                        consumed = record.consumed_count;
                    }
                }
            }
        }
        Ok(Self {
            file_path,
            consumed: AtomicU64::new(consumed),
            limit,
            lock: Mutex::new(()),
        })
    }

    /// Create an in-memory budget manager (useful for testing or ephemeral runs).
    pub fn new_in_memory(limit: u64) -> Self {
        Self {
            file_path: PathBuf::new(),
            consumed: AtomicU64::new(0),
            limit,
            lock: Mutex::new(()),
        }
    }

    pub fn consumed(&self) -> u64 {
        self.consumed.load(Ordering::Acquire)
    }

    pub fn remaining(&self) -> u64 {
        let current = self.consumed();
        self.limit.saturating_sub(current)
    }

    pub fn is_exhausted(&self) -> bool {
        self.consumed() >= self.limit
    }

    /// Pre-allocates and persistently increments the budget before sending (ADR 0102).
    /// Returns the new consumed count upon success.
    pub fn reserve(&self) -> Result<u64, DecisionsError> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| DecisionsError::PersistenceFailed)?;
        let current = self.consumed.load(Ordering::Acquire);
        if current >= self.limit {
            return Err(DecisionsError::BudgetExhausted);
        }
        let next_count = current + 1;

        // If file_path is not empty, persistently store the updated count atomically
        if !self.file_path.as_os_str().is_empty() {
            let record = StoredBudget {
                version: 1,
                consumed_count: next_count,
                limit: self.limit,
            };
            let json = serde_json::to_string_pretty(&record)
                .map_err(|_| DecisionsError::PersistenceFailed)?;

            let tmp_path = self.file_path.with_extension("tmp");
            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&tmp_path)
                .map_err(|_| DecisionsError::PersistenceFailed)?;
            file.write_all(json.as_bytes())
                .map_err(|_| DecisionsError::PersistenceFailed)?;
            file.flush()
                .map_err(|_| DecisionsError::PersistenceFailed)?;
            drop(file);

            fs::rename(&tmp_path, &self.file_path)
                .map_err(|_| DecisionsError::PersistenceFailed)?;
        }

        self.consumed.store(next_count, Ordering::Release);
        Ok(next_count)
    }
}

/// Build the OpenAI API request body for decisions evaluation.
pub fn request_body_decisions(
    model: &str,
    states: &[DecisionState],
) -> Result<Vec<u8>, DecisionsError> {
    if !crate::first_conversion::valid_model(model) {
        return Err(DecisionsError::InvalidModel);
    }
    if states.is_empty() {
        return Err(DecisionsError::EmptyStates);
    }
    if states.len() > MAX_STATES_PER_BATCH {
        return Err(DecisionsError::TooManyStates);
    }

    for state in states {
        if state.text.graphemes(true).count() > MAX_GRAPHEMES_PER_STATE {
            return Err(DecisionsError::GraphemesExceeded);
        }
    }

    let user_content =
        serde_json::to_string(states).map_err(|_| DecisionsError::InvalidResponse)?;

    let request_payload = serde_json::json!({
        "model": model,
        "temperature": 0.0,
        "messages": [
            {
                "role": "system",
                "content": DECISIONS_SYSTEM_PROMPT
            },
            {
                "role": "user",
                "content": user_content
            }
        ],
        "response_format": {
            "type": "json_object"
        }
    });

    serde_json::to_vec(&request_payload).map_err(|_| DecisionsError::InvalidResponse)
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: AssistantMessage,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct AssistantMessage {
    role: String,
    content: Option<String>,
    refusal: Option<serde_json::Value>,
}

/// Parse and validate Decisions API response bytes against expected state IDs (ADR 0102).
pub fn response_decisions(
    bytes: &[u8],
    expected_ids: &[u64],
) -> Result<Vec<DecisionResultItem>, DecisionsError> {
    if bytes.is_empty() {
        return Err(DecisionsError::InvalidResponse);
    }
    let chat_response: ChatResponse =
        serde_json::from_slice(bytes).map_err(|_| DecisionsError::InvalidResponse)?;
    if chat_response.choices.len() != 1 {
        return Err(DecisionsError::InvalidResponse);
    }
    let choice = &chat_response.choices[0];
    if choice.finish_reason.as_deref() != Some("stop") {
        return Err(DecisionsError::InvalidResponse);
    }
    if choice.message.role != "assistant" || choice.message.refusal.is_some() {
        return Err(DecisionsError::InvalidResponse);
    }
    let content = choice
        .message
        .content
        .as_deref()
        .ok_or(DecisionsError::InvalidResponse)?;

    let decision_resp: DecisionResponse =
        serde_json::from_str(content).map_err(|_| DecisionsError::InvalidResponse)?;

    validate_decision_response(expected_ids, &decision_resp)
        .map_err(|_| DecisionsError::InvalidResponse)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_budget_manager_reserve_and_exhaustion() {
        let budget = BudgetManager::new_in_memory(3);
        assert_eq!(budget.consumed(), 0);
        assert_eq!(budget.remaining(), 3);
        assert!(!budget.is_exhausted());

        assert_eq!(budget.reserve().unwrap(), 1);
        assert_eq!(budget.consumed(), 1);
        assert_eq!(budget.remaining(), 2);

        assert_eq!(budget.reserve().unwrap(), 2);
        assert_eq!(budget.reserve().unwrap(), 3);
        assert!(budget.is_exhausted());

        // 4th request must fail with BudgetExhausted
        assert_eq!(budget.reserve(), Err(DecisionsError::BudgetExhausted));
        assert_eq!(budget.consumed(), 3);
    }

    #[test]
    fn test_budget_manager_persistence() {
        let tmp_dir =
            std::env::temp_dir().join(format!("sumibi_test_budget_{}", std::process::id()));
        let _ = fs::create_dir_all(&tmp_dir);
        let budget_file = tmp_dir.join("decisions_budget.json");

        {
            let budget = BudgetManager::new(budget_file.clone(), 5).unwrap();
            assert_eq!(budget.reserve().unwrap(), 1);
            assert_eq!(budget.reserve().unwrap(), 2);
        }

        // Reopen from disk: must preserve consumed count
        {
            let budget = BudgetManager::new(budget_file.clone(), 5).unwrap();
            assert_eq!(budget.consumed(), 2);
            assert_eq!(budget.reserve().unwrap(), 3);
        }

        let _ = fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn test_request_body_decisions_validation() {
        let states = vec![
            DecisionState::new(1, "sumibi", 1000),
            DecisionState::new(2, "desu", 1200),
        ];

        for model in ["gpt-4o-mini"]
            .into_iter()
            .chain(crate::provider::GEMINI_MODELS)
        {
            let body = request_body_decisions(model, &states).unwrap();
            let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(json["model"], model);
            assert_eq!(json["temperature"], 0.0);
            assert_eq!(json["response_format"]["type"], "json_object");
        }

        // Rejects empty states
        assert_eq!(
            request_body_decisions("gpt-4o-mini", &[]),
            Err(DecisionsError::EmptyStates)
        );

        // Rejects invalid model
        assert_eq!(
            request_body_decisions("", &states),
            Err(DecisionsError::InvalidModel)
        );
    }

    #[test]
    fn test_response_decisions_parsing() {
        let expected_ids = [1, 2];
        let raw_response = serde_json::json!({
            "choices": [{
                "finish_reason": "stop",
                "message": {
                    "role": "assistant",
                    "content": r#"{"decisions":[{"id":1,"should_convert":true,"confidence":0.95,"converted_text":"炭火"},{"id":2,"should_convert":false,"confidence":0.1}]}"#
                }
            }]
        });

        let bytes = serde_json::to_vec(&raw_response).unwrap();
        let items = response_decisions(&bytes, &expected_ids).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, 1);
        assert!(items[0].should_convert);
        assert_eq!(items[0].converted_text.as_deref(), Some("炭火"));
        assert_eq!(items[1].id, 2);
        assert!(!items[1].should_convert);
    }
}
