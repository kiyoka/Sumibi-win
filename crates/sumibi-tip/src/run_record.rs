//! Passive record of the romaji run the user just typed (ADR 0104).
//!
//! The record never leaves the TIP on its own and is never used as a source of
//! document text: at conversion time the document is read back for exactly the
//! recorded length and must match, otherwise the conversion is refused. Every
//! boundary (focus, caret keys, Enter, Ctrl/Alt combos, non-romaji input,
//! overflow) discards the record. Contents are overwritten with zeros on drop.

/// Upper bound of one conversion unit, in UTF-16 units.
pub(crate) const MAX_RUN_UNITS: usize = 256;

/// What a single observed key press does to the record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeyAction {
    Push(u16),
    Backspace,
    Clear,
    Ignore,
}

const VK_BACK: u32 = 0x08;

impl KeyAction {
    /// Fixed label for diagnostics. Never contains the typed character.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Push(_) => "push",
            Self::Backspace => "backspace",
            Self::Clear => "clear",
            Self::Ignore => "ignore",
        }
    }
}

/// Modifier keys pressed on their own never change the document. Ctrl in
/// particular must not clear the record: it precedes the Ctrl+J hotkey.
fn is_pure_modifier(vk: u32) -> bool {
    matches!(vk, 0x10 | 0x11 | 0x12 | 0x14 | 0xA0..=0xA5 | 0x5B | 0x5C)
}

fn is_romaji_unit(unit: u16) -> bool {
    matches!(unit,
        0x30..=0x39 | 0x41..=0x5A | 0x61..=0x7A
        | 0x20 | 0x2D | 0x27 | 0x2C | 0x2E | 0x3F | 0x21)
}

/// Classify one key press. `character` is the layout translation of the key
/// with the current shift/caps state, or `None` for non-character keys.
pub(crate) fn classify(vk: u32, character: Option<u16>, command_modifier: bool) -> KeyAction {
    if is_pure_modifier(vk) {
        return KeyAction::Ignore;
    }
    if command_modifier {
        return KeyAction::Clear;
    }
    if vk == VK_BACK {
        return KeyAction::Backspace;
    }
    match character {
        Some(unit) if is_romaji_unit(unit) => KeyAction::Push(unit),
        _ => KeyAction::Clear,
    }
}

#[derive(Default)]
pub(crate) struct RunRecord {
    context: usize,
    units: Vec<u16>,
    overflowed: bool,
}

impl RunRecord {
    pub(crate) fn len(&self) -> usize {
        self.units.len()
    }

    pub(crate) fn clear(&mut self) {
        self.units.iter_mut().for_each(|unit| *unit = 0);
        self.units.clear();
        self.context = 0;
        self.overflowed = false;
    }

    /// `context` is an identity token only (never dereferenced).
    pub(crate) fn apply(&mut self, context: usize, action: KeyAction) {
        if action == KeyAction::Ignore {
            return;
        }
        if action == KeyAction::Clear || context == 0 {
            self.clear();
            return;
        }
        if self.context != context {
            self.clear();
            self.context = context;
        }
        match action {
            KeyAction::Push(unit) => {
                if self.overflowed {
                    return;
                }
                if unit == 0x20 && self.units.is_empty() {
                    return;
                }
                if self.units.len() >= MAX_RUN_UNITS {
                    // Too long to be one unit; ignore until the next boundary
                    // instead of starting a fresh run in the middle of a word.
                    self.units.iter_mut().for_each(|unit| *unit = 0);
                    self.units.clear();
                    self.overflowed = true;
                    return;
                }
                self.units.push(unit);
            }
            KeyAction::Backspace => {
                self.units.pop();
            }
            KeyAction::Clear | KeyAction::Ignore => {}
        }
    }

    /// Consume the record for `context`. It is always discarded afterwards,
    /// whether or not the conversion later succeeds.
    pub(crate) fn take(&mut self, context: usize) -> Option<Vec<u16>> {
        let usable =
            context != 0 && self.context == context && !self.overflowed && !self.units.is_empty();
        let taken = if usable {
            Some(std::mem::take(&mut self.units))
        } else {
            None
        };
        self.clear();
        taken
    }

    #[cfg(test)]
    fn text(&self) -> String {
        String::from_utf16_lossy(&self.units)
    }
}

impl Drop for RunRecord {
    fn drop(&mut self) {
        self.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push_str(record: &mut RunRecord, context: usize, text: &str) {
        for unit in text.encode_utf16() {
            record.apply(context, KeyAction::Push(unit));
        }
    }

    #[test]
    fn classifies_romaji_and_boundaries() {
        assert_eq!(
            classify(0x41, Some(b'a' as u16), false),
            KeyAction::Push(0x61)
        );
        assert_eq!(classify(0x20, Some(0x20), false), KeyAction::Push(0x20));
        assert_eq!(classify(0x08, None, false), KeyAction::Backspace);
        assert_eq!(classify(0x0D, Some(0x0D), false), KeyAction::Clear);
        assert_eq!(classify(0x25, None, false), KeyAction::Clear);
        assert_eq!(classify(0x28, None, false), KeyAction::Clear);
        assert_eq!(classify(0x41, Some(0x61), true), KeyAction::Clear);
        assert_eq!(classify(0x11, None, true), KeyAction::Ignore);
        assert_eq!(classify(0x10, None, false), KeyAction::Ignore);
        assert_eq!(classify(0x14, None, false), KeyAction::Ignore);
        assert_eq!(classify(0xBA, Some(0x28), false), KeyAction::Clear);
        assert_eq!(classify(0x70, None, false), KeyAction::Clear);
        assert_eq!(classify(0x41, None, false), KeyAction::Clear);
    }

    #[test]
    fn records_edits_and_takes_once() {
        let mut record = RunRecord::default();
        push_str(&mut record, 7, "nihonngo");
        record.apply(7, KeyAction::Backspace);
        assert_eq!(record.text(), "nihonng");
        let taken = record.take(7).unwrap();
        assert_eq!(String::from_utf16(&taken).unwrap(), "nihonng");
        assert!(record.take(7).is_none());
    }

    #[test]
    fn leading_space_is_not_recorded_but_inner_space_is() {
        let mut record = RunRecord::default();
        push_str(&mut record, 1, " ");
        assert_eq!(record.text(), "");
        push_str(&mut record, 1, "ab cd");
        assert_eq!(record.text(), "ab cd");
    }

    #[test]
    fn context_change_and_clear_discard_the_run() {
        let mut record = RunRecord::default();
        push_str(&mut record, 1, "abc");
        assert!(record.take(2).is_none());
        push_str(&mut record, 1, "abc");
        record.apply(1, KeyAction::Clear);
        assert!(record.take(1).is_none());
        push_str(&mut record, 1, "abc");
        push_str(&mut record, 2, "x");
        assert_eq!(record.text(), "x");
        assert!(record.take(0).is_none());
    }

    #[test]
    fn overflow_poisons_the_run_until_a_boundary() {
        let mut record = RunRecord::default();
        push_str(&mut record, 1, &"a".repeat(MAX_RUN_UNITS));
        assert_eq!(record.text().len(), MAX_RUN_UNITS);
        push_str(&mut record, 1, "b");
        assert!(record.take(1).is_none());
        push_str(&mut record, 1, &"a".repeat(MAX_RUN_UNITS + 1));
        push_str(&mut record, 1, "c");
        assert_eq!(record.text(), "");
        record.apply(1, KeyAction::Clear);
        push_str(&mut record, 1, "ok");
        assert_eq!(record.text(), "ok");
    }

    #[test]
    fn ignore_keeps_the_run() {
        let mut record = RunRecord::default();
        push_str(&mut record, 3, "abc");
        record.apply(3, KeyAction::Ignore);
        assert_eq!(record.text(), "abc");
    }
}
