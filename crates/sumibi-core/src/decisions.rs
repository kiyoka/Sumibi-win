//! Decisions auto-conversion data models, buffer limits and response validation.
//! Implements ADR 0102: separate consent, 200ms fixed window, strict schema validation, and cost protection.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use unicode_segmentation::UnicodeSegmentation;

/// Fixed window duration for batching observation states (200 milliseconds).
pub const WINDOW_DURATION_MS: u64 = 200;

/// Maximum number of observation states sent in a single batch.
pub const MAX_STATES_PER_BATCH: usize = 32;

/// Maximum number of buffered pending observation states.
pub const MAX_PENDING_STATES: usize = 256;

/// Maximum grapheme clusters allowed in a single observation state.
pub const MAX_GRAPHEMES_PER_STATE: usize = 512;

/// Confidence threshold for adopting an auto-conversion suggestion.
pub const CONFIDENCE_THRESHOLD: f64 = 0.5;

/// Initial lifetime budget cap for decisions auto-conversion requests (10,000 requests).
pub const MAX_DECISIONS_BUDGET: u64 = 10_000;

/// Individual text observation state tracked for decision evaluation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionState {
    pub id: u64,
    pub text: String,
    pub timestamp_ms: u64,
}

impl DecisionState {
    pub fn new(id: u64, text: impl Into<String>, timestamp_ms: u64) -> Self {
        Self {
            id,
            text: text.into(),
            timestamp_ms,
        }
    }
}

/// Single decision evaluation result item returned by the model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DecisionResultItem {
    pub id: u64,
    pub should_convert: bool,
    pub confidence: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub converted_text: Option<String>,
}

/// Top-level response container matching the structured Decisions schema.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DecisionResponse {
    pub decisions: Vec<DecisionResultItem>,
}

/// Errors during Decision response validation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecisionError {
    /// Payload is not valid JSON or does not conform to the schema.
    MalformedJson,
    /// Result count does not match the expected request state count.
    CountMismatch,
    /// Response contains duplicate state IDs.
    DuplicateId,
    /// Response contains an unknown ID not present in the request batch.
    UnknownId,
    /// A required ID from the request batch was omitted.
    MissingId,
    /// Confidence value is not a finite floating-point number between 0.0 and 1.0.
    InvalidConfidence,
    /// Conversion flag is true, but converted_text is empty or missing.
    MissingConvertedText,
}

/// Strictly validates the decisions response against the expected state IDs (ADR 0102).
///
/// Rules:
/// 1. Exactly matches the expected set of IDs (order-independent).
/// 2. No duplicate IDs.
/// 3. `confidence` must be finite and within `0.0..=1.0`.
/// 4. If `should_convert` is true, `converted_text` must be present and non-empty.
pub fn validate_decision_response(
    expected_ids: &[u64],
    response: &DecisionResponse,
) -> Result<Vec<DecisionResultItem>, DecisionError> {
    if response.decisions.len() != expected_ids.len() {
        return Err(DecisionError::CountMismatch);
    }

    let mut seen_ids = std::collections::HashSet::with_capacity(expected_ids.len());
    for item in &response.decisions {
        // Check duplicate
        if !seen_ids.insert(item.id) {
            return Err(DecisionError::DuplicateId);
        }

        // Check ID belongs to expected batch
        if !expected_ids.contains(&item.id) {
            return Err(DecisionError::UnknownId);
        }

        // Check confidence is a finite value in 0.0..=1.0
        if !item.confidence.is_finite() || item.confidence < 0.0 || item.confidence > 1.0 {
            return Err(DecisionError::InvalidConfidence);
        }

        // If should_convert is true, converted_text must exist and be valid
        if item.should_convert {
            match &item.converted_text {
                Some(text) if !text.trim().is_empty() => {}
                _ => return Err(DecisionError::MissingConvertedText),
            }
        }
    }

    // Verify all expected IDs were present
    for expected_id in expected_ids {
        if !seen_ids.contains(expected_id) {
            return Err(DecisionError::MissingId);
        }
    }

    Ok(response.decisions.clone())
}

/// A decision replacement ready to be applied if confidence threshold is met.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecisionReplacement {
    pub id: u64,
    pub converted_text: String,
}

/// Buffer and batching state machine for Decisions auto-conversion (ADR 0102).
#[derive(Debug)]
pub struct DecisionsBuffer {
    pending: VecDeque<DecisionState>,
    next_id: u64,
    current_generation: u64,
    window_start_ms: Option<u64>,
}

impl Default for DecisionsBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl DecisionsBuffer {
    pub fn new() -> Self {
        Self {
            pending: VecDeque::with_capacity(MAX_PENDING_STATES),
            next_id: 1,
            current_generation: 1,
            window_start_ms: None,
        }
    }

    pub fn current_generation(&self) -> u64 {
        self.current_generation
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Push an observed text state.
    /// Rejects if text exceeds 512 graphemes or is empty.
    /// Evicts the oldest state if pending buffer capacity (256) is reached.
    /// Returns the assigned state ID if pushed.
    pub fn push_state(&mut self, text: impl Into<String>, timestamp_ms: u64) -> Option<u64> {
        let text = text.into();
        let grapheme_count = text.graphemes(true).count();
        if grapheme_count == 0 || grapheme_count > MAX_GRAPHEMES_PER_STATE {
            return None;
        }

        if self.pending.len() >= MAX_PENDING_STATES {
            self.pending.pop_front();
        }

        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        if self.next_id == 0 {
            self.next_id = 1;
        }

        self.pending
            .push_back(DecisionState::new(id, text, timestamp_ms));
        if self.window_start_ms.is_none() {
            self.window_start_ms = Some(timestamp_ms);
        }

        Some(id)
    }

    /// Check if the fixed window duration (200ms) has elapsed since the earliest buffered state.
    pub fn is_window_elapsed(&self, now_ms: u64) -> bool {
        if let Some(start) = self.window_start_ms {
            now_ms.saturating_sub(start) >= WINDOW_DURATION_MS && !self.pending.is_empty()
        } else {
            false
        }
    }

    /// Drain up to MAX_STATES_PER_BATCH (32) pending states for model evaluation.
    /// Resets the window timer for remaining states if any.
    pub fn drain_batch(&mut self, now_ms: u64) -> (u64, Vec<DecisionState>) {
        let count = self.pending.len().min(MAX_STATES_PER_BATCH);
        let batch: Vec<DecisionState> = self.pending.drain(..count).collect();

        if self.pending.is_empty() {
            self.window_start_ms = None;
        } else {
            self.window_start_ms = Some(now_ms);
        }

        (self.current_generation, batch)
    }

    /// Invalidate pending states and bump generation on manual conversion trigger (`Ctrl+J`).
    /// All in-flight or buffered auto-conversion requests from prior generations are discarded.
    pub fn invalidate_manual_conversion(&mut self) {
        self.pending.clear();
        self.window_start_ms = None;
        self.current_generation = self.current_generation.wrapping_add(1);
        if self.current_generation == 0 {
            self.current_generation = 1;
        }
    }

    /// Evaluate an individual result item against generation and confidence threshold (0.5).
    pub fn evaluate_item(
        &self,
        generation: u64,
        item: &DecisionResultItem,
    ) -> Option<DecisionReplacement> {
        if generation != self.current_generation {
            return None;
        }
        if item.should_convert && item.confidence >= CONFIDENCE_THRESHOLD {
            if let Some(ref text) = item.converted_text {
                if !text.trim().is_empty() {
                    return Some(DecisionReplacement {
                        id: item.id,
                        converted_text: text.clone(),
                    });
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_decision_response() {
        let expected = vec![1, 2, 3];
        let resp = DecisionResponse {
            decisions: vec![
                DecisionResultItem {
                    id: 2,
                    should_convert: false,
                    confidence: 0.1,
                    converted_text: None,
                },
                DecisionResultItem {
                    id: 1,
                    should_convert: true,
                    confidence: 0.95,
                    converted_text: Some("日本語".to_string()),
                },
                DecisionResultItem {
                    id: 3,
                    should_convert: true,
                    confidence: 0.7,
                    converted_text: Some("漢字".to_string()),
                },
            ],
        };

        let validated = validate_decision_response(&expected, &resp).unwrap();
        assert_eq!(validated.len(), 3);
    }

    #[test]
    fn test_rejects_duplicate_id() {
        let expected = vec![1, 2];
        let resp = DecisionResponse {
            decisions: vec![
                DecisionResultItem {
                    id: 1,
                    should_convert: false,
                    confidence: 0.1,
                    converted_text: None,
                },
                DecisionResultItem {
                    id: 1,
                    should_convert: false,
                    confidence: 0.2,
                    converted_text: None,
                },
            ],
        };
        assert_eq!(
            validate_decision_response(&expected, &resp),
            Err(DecisionError::DuplicateId)
        );
    }

    #[test]
    fn test_rejects_unknown_and_missing_ids() {
        let expected = vec![1, 2];
        let resp = DecisionResponse {
            decisions: vec![
                DecisionResultItem {
                    id: 1,
                    should_convert: false,
                    confidence: 0.1,
                    converted_text: None,
                },
                DecisionResultItem {
                    id: 999, // unknown ID
                    should_convert: false,
                    confidence: 0.2,
                    converted_text: None,
                },
            ],
        };
        assert_eq!(
            validate_decision_response(&expected, &resp),
            Err(DecisionError::UnknownId)
        );
    }

    #[test]
    fn test_rejects_invalid_confidence() {
        let expected = vec![1];
        for bad_conf in [-0.01, 1.01, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let resp = DecisionResponse {
                decisions: vec![DecisionResultItem {
                    id: 1,
                    should_convert: false,
                    confidence: bad_conf,
                    converted_text: None,
                }],
            };
            assert_eq!(
                validate_decision_response(&expected, &resp),
                Err(DecisionError::InvalidConfidence)
            );
        }
    }

    #[test]
    fn test_rejects_missing_converted_text_when_should_convert_true() {
        let expected = vec![1];
        let resp = DecisionResponse {
            decisions: vec![DecisionResultItem {
                id: 1,
                should_convert: true,
                confidence: 0.9,
                converted_text: None, // missing
            }],
        };
        assert_eq!(
            validate_decision_response(&expected, &resp),
            Err(DecisionError::MissingConvertedText)
        );

        let resp_empty = DecisionResponse {
            decisions: vec![DecisionResultItem {
                id: 1,
                should_convert: true,
                confidence: 0.9,
                converted_text: Some("   ".to_string()), // empty / whitespace only
            }],
        };
        assert_eq!(
            validate_decision_response(&expected, &resp_empty),
            Err(DecisionError::MissingConvertedText)
        );
    }

    #[test]
    fn test_decisions_buffer_push_and_overflow() {
        let mut buffer = DecisionsBuffer::new();
        assert_eq!(buffer.pending_count(), 0);

        for i in 0..300 {
            let id = buffer.push_state(format!("text_{i}"), i as u64 * 10);
            assert!(id.is_some());
        }

        // Must be capped at MAX_PENDING_STATES (256)
        assert_eq!(buffer.pending_count(), MAX_PENDING_STATES);

        // Oldest states (0..44) were evicted, first pending state should have id 45
        let (_, batch) = buffer.drain_batch(5000);
        assert_eq!(batch.len(), MAX_STATES_PER_BATCH);
        assert_eq!(batch[0].id, 45);
    }

    #[test]
    fn test_decisions_buffer_fixed_window_and_batch_drain() {
        let mut buffer = DecisionsBuffer::new();
        assert!(!buffer.is_window_elapsed(100));

        // Push first state at t=100
        buffer.push_state("nihon", 100);

        // At t=250 (<200ms elapsed since 100), window is not elapsed
        assert!(!buffer.is_window_elapsed(250));

        // At t=300 (200ms elapsed), window is elapsed
        assert!(buffer.is_window_elapsed(300));

        // Drain batch
        let (gen, batch) = buffer.drain_batch(300);
        assert_eq!(gen, 1);
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].text, "nihon");

        // After drain, buffer is empty, so window is not elapsed
        assert!(!buffer.is_window_elapsed(400));
    }

    #[test]
    fn test_decisions_buffer_grapheme_limits() {
        let mut buffer = DecisionsBuffer::new();

        // Empty string rejected
        assert_eq!(buffer.push_state("", 100), None);

        // Exactly 512 graphemes accepted
        let s512 = "あ".repeat(512);
        assert!(buffer.push_state(&s512, 100).is_some());

        // 513 graphemes rejected
        let s513 = "あ".repeat(513);
        assert_eq!(buffer.push_state(&s513, 100), None);
    }

    #[test]
    fn test_decisions_buffer_manual_invalidation() {
        let mut buffer = DecisionsBuffer::new();
        buffer.push_state("test1", 100);
        buffer.push_state("test2", 150);
        assert_eq!(buffer.pending_count(), 2);
        let initial_gen = buffer.current_generation();

        // Manual conversion invalidation clears buffer and bumps generation
        buffer.invalidate_manual_conversion();
        assert_eq!(buffer.pending_count(), 0);
        assert_ne!(buffer.current_generation(), initial_gen);

        // Stale generation result is rejected
        let item = DecisionResultItem {
            id: 1,
            should_convert: true,
            confidence: 0.99,
            converted_text: Some("テスト".to_string()),
        };
        assert_eq!(buffer.evaluate_item(initial_gen, &item), None);
    }

    #[test]
    fn test_decisions_buffer_evaluate_item_threshold() {
        let buffer = DecisionsBuffer::new();
        let gen = buffer.current_generation();

        // Confidence 0.50 (at threshold) and should_convert: true -> accepted
        let item_ok = DecisionResultItem {
            id: 1,
            should_convert: true,
            confidence: 0.50,
            converted_text: Some("テスト".to_string()),
        };
        assert_eq!(
            buffer.evaluate_item(gen, &item_ok),
            Some(DecisionReplacement {
                id: 1,
                converted_text: "テスト".to_string(),
            })
        );

        // Confidence 0.499 (< 0.5) -> rejected
        let item_low = DecisionResultItem {
            id: 2,
            should_convert: true,
            confidence: 0.499,
            converted_text: Some("テスト".to_string()),
        };
        assert_eq!(buffer.evaluate_item(gen, &item_low), None);

        // should_convert: false -> rejected even if confidence is 1.0
        let item_no_convert = DecisionResultItem {
            id: 3,
            should_convert: false,
            confidence: 1.0,
            converted_text: Some("テスト".to_string()),
        };
        assert_eq!(buffer.evaluate_item(gen, &item_no_convert), None);
    }
}
