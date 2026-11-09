//! Candidate list session and keyboard navigation on the STA thread.
//! Implements ADR 0101: strict 1-step undo guarantee and clean source separation.

use sumibi_core::candidate::CandidateItem;

/// Active candidate selection session held by TIP presentation layer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CandidateSession {
    /// Ordered candidate list (Index 0 is the current/initial conversion result).
    pub candidates: Vec<CandidateItem>,
    /// Currently focused candidate index (0 <= selected_index < candidates.len()).
    pub selected_index: usize,
    /// Whether the candidate popup / presenter is currently visible and accepting keys.
    pub is_active: bool,
    /// Original pre-conversion source text (e.g. "sumibi").
    pub original_text: String,
    /// Currently applied result text in document (e.g. "炭火").
    pub current_result: String,
    /// Host policy / session generation counter.
    pub generation: u64,
}

impl CandidateSession {
    /// Create a new active candidate session.
    pub fn new(
        original_text: String,
        current_result: String,
        candidates: Vec<CandidateItem>,
        generation: u64,
    ) -> Self {
        let is_active = !candidates.is_empty();
        Self {
            candidates,
            selected_index: 0,
            is_active,
            original_text,
            current_result,
            generation,
        }
    }

    /// Whether the candidate session is active.
    pub fn is_active(&self) -> bool {
        self.is_active && !self.candidates.is_empty()
    }

    /// Number of candidates.
    pub fn len(&self) -> usize {
        self.candidates.len()
    }

    /// Whether there are no candidates.
    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    /// Currently selected candidate item.
    pub fn selected_candidate(&self) -> Option<&CandidateItem> {
        self.candidates.get(self.selected_index)
    }

    /// Move focus to next candidate (Down arrow / Tab), wrapping around.
    pub fn next(&mut self) {
        if self.candidates.is_empty() {
            return;
        }
        self.selected_index = (self.selected_index + 1) % self.candidates.len();
    }

    /// Move focus to previous candidate (Up arrow / Shift+Tab), wrapping around.
    pub fn prev(&mut self) {
        if self.candidates.is_empty() {
            return;
        }
        self.selected_index =
            (self.selected_index + self.candidates.len() - 1) % self.candidates.len();
    }

    /// Select directly by digit key ('1'..='9', '0').
    ///
    /// Mapping:
    /// '1' -> Index 0
    /// '2' -> Index 1
    /// ...
    /// '9' -> Index 8
    /// '0' -> Index 9
    ///
    /// Returns the selected CandidateItem if the index is valid.
    pub fn select_digit(&mut self, digit: char) -> Option<&CandidateItem> {
        self.selected_index = self.digit_index(digit)?;
        self.selected_candidate()
    }

    pub(crate) fn digit_index(&self, digit: char) -> Option<usize> {
        let index = match digit {
            '1'..='9' => (digit as usize) - ('1' as usize),
            '0' => 9,
            _ => return None,
        };
        (index < self.candidates.len()).then_some(index)
    }

    /// Confirm selection and finish the session.
    ///
    /// Returns pure text to replace the document with.
    /// Annotations (e.g. "[記号]", "[辞書]") are strictly excluded from the document text.
    pub fn confirm(&mut self) -> Option<String> {
        if !self.is_active() {
            return None;
        }
        let text = self
            .candidates
            .get(self.selected_index)
            .map(|item| item.text.clone());
        self.is_active = false;
        text
    }

    /// Cancel the candidate session without replacing text.
    pub fn cancel(&mut self) {
        self.is_active = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sumibi_core::candidate::CandidateSource;

    fn sample_candidates() -> Vec<CandidateItem> {
        vec![
            CandidateItem::new("炭火", CandidateSource::CurrentResult),
            CandidateItem::new("澄火", CandidateSource::Llm),
            CandidateItem::with_annotation("・", CandidateSource::Nakaguro, "中黒"),
            CandidateItem::with_annotation(" charcoal ", CandidateSource::MozcSymbol, "記号"),
        ]
    }

    #[test]
    fn test_navigation_and_wrapping() {
        let mut session = CandidateSession::new(
            "sumibi".to_string(),
            "炭火".to_string(),
            sample_candidates(),
            1,
        );

        assert!(session.is_active());
        assert_eq!(session.selected_index, 0);
        assert_eq!(session.selected_candidate().unwrap().text, "炭火");

        session.next();
        assert_eq!(session.selected_index, 1);
        assert_eq!(session.selected_candidate().unwrap().text, "澄火");

        session.prev();
        assert_eq!(session.selected_index, 0);

        // Wrap backwards
        session.prev();
        assert_eq!(session.selected_index, 3);
        assert_eq!(session.selected_candidate().unwrap().text, " charcoal ");

        // Wrap forwards
        session.next();
        assert_eq!(session.selected_index, 0);
    }

    #[test]
    fn test_digit_selection() {
        let mut session = CandidateSession::new(
            "sumibi".to_string(),
            "炭火".to_string(),
            sample_candidates(),
            1,
        );

        // '1' selects index 0
        assert_eq!(session.select_digit('1').unwrap().text, "炭火");
        assert_eq!(session.selected_index, 0);

        // '3' selects index 2 ("・")
        assert_eq!(session.select_digit('3').unwrap().text, "・");
        assert_eq!(session.selected_index, 2);

        // '5' is out of bounds (only 4 candidates)
        assert!(session.select_digit('5').is_none());
        // selected_index remains unchanged
        assert_eq!(session.selected_index, 2);
    }

    #[test]
    fn test_confirm_excludes_annotation() {
        let mut session = CandidateSession::new(
            "sumibi".to_string(),
            "炭火".to_string(),
            sample_candidates(),
            1,
        );

        session.select_digit('4'); // " charcoal " with annotation "記号"
        assert_eq!(session.selected_index, 3);

        let confirmed_text = session.confirm();
        assert_eq!(confirmed_text, Some(" charcoal ".to_string()));
        assert!(!session.is_active());
    }

    #[test]
    fn test_cancel() {
        let mut session = CandidateSession::new(
            "sumibi".to_string(),
            "炭火".to_string(),
            sample_candidates(),
            1,
        );

        session.cancel();
        assert!(!session.is_active());
        assert_eq!(session.confirm(), None);
    }
}
