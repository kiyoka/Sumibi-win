//! Bounded tracking of text actually inserted by a cooperating host. This does
//! not consume events, edit a document, or infer pre-existing text from a caret.
pub const MAX_TRACKED_UNITS: usize = 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct InputPoint {
    pub context: u128,
    pub generation: u64,
    pub composition_generation: u64,
    pub selection: (i32, i32),
    pub allowed: bool,
}
struct Run {
    start: i32,
    point: InputPoint,
    text: Vec<u16>,
}
/// Text-bearing types intentionally omit Debug. No state is persisted.
#[derive(Default)]
pub struct InputSession {
    run: Option<Run>,
}
pub struct TrackedInput {
    start: i32,
    end: i32,
    text: Vec<u16>,
}
impl TrackedInput {
    pub fn range(&self) -> (i32, i32) {
        (self.start, self.end)
    }
    pub fn matches(&self, text: &[u16]) -> bool {
        self.text == text
    }
    pub fn text(&self) -> &[u16] {
        &self.text
    }
}
fn valid(point: InputPoint) -> bool {
    point.allowed
        && point.context != 0
        && point.generation != u64::MAX
        && point.composition_generation != u64::MAX
        && point.selection.0 >= 0
        && point.selection.1 >= point.selection.0
}
impl InputSession {
    /// Metadata for an exact live observation, without consuming private text.
    pub fn range(&self, current: InputPoint) -> Option<(i32, i32)> {
        let run = self.run.as_ref()?;
        (valid(current) && current == run.point).then_some((run.start, run.point.selection.1))
    }
    pub fn clear(&mut self) {
        self.run = None;
    }
    /// Call only after the original host insertion succeeds. Failure here is
    /// loss of tracking, never permission to undo/replay/drop that input event.
    pub fn inserted(&mut self, before: InputPoint, after: InputPoint, text: &[u16]) {
        let previous = self.run.take();
        if !valid(before)
            || !valid(after)
            || before.context != after.context
            || after.generation <= before.generation
            || before.composition_generation != after.composition_generation
            || text.is_empty()
            || text.len() > MAX_TRACKED_UNITS
            || char::decode_utf16(text.iter().copied())
                .any(|unit| unit.map_or(true, char::is_control))
        {
            return;
        }
        let Some(end) = before.selection.0.checked_add(text.len() as i32) else {
            return;
        };
        if after.selection != (end, end) {
            return;
        }
        let mut run = match previous {
            Some(run) if run.point == before && before.selection.0 == before.selection.1 => run,
            _ => Run {
                start: before.selection.0,
                point: before,
                text: Vec::new(),
            },
        };
        if run.text.len() + text.len() > MAX_TRACKED_UNITS {
            return;
        }
        run.text.extend_from_slice(text);
        run.point = after;
        self.run = Some(run);
    }
    /// A cooperating host already deleted a suffix. Never infer surrounding
    /// text or cross the tracked start; a split surrogate retires the run.
    pub fn backspaced(&mut self, before: InputPoint, after: InputPoint) {
        let Some(mut run) = self.run.take() else {
            return;
        };
        if !valid(before)
            || !valid(after)
            || before != run.point
            || before.context != after.context
            || after.generation <= before.generation
            || before.composition_generation != after.composition_generation
            || before.selection.0 != before.selection.1
            || after.selection.0 != after.selection.1
            || after.selection.0 < run.start
            || after.selection.0 >= before.selection.0
        {
            return;
        }
        let remaining = (after.selection.0 - run.start) as usize;
        if remaining == 0 || remaining >= run.text.len() {
            return;
        }
        run.text.truncate(remaining);
        if String::from_utf16(&run.text).is_err() {
            return;
        }
        run.point = after;
        self.run = Some(run);
    }
    /// Consume before selection changes or transport calls. A failed match can
    /// never be retried against another field or guessed nearby text.
    pub fn take(&mut self, current: InputPoint) -> Option<TrackedInput> {
        let run = self.run.take()?;
        if !valid(current) || current != run.point {
            return None;
        }
        Some(TrackedInput {
            start: run.start,
            end: run.point.selection.1,
            text: run.text,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn point(position: i32, generation: u64) -> InputPoint {
        InputPoint {
            context: 1,
            generation,
            composition_generation: 0,
            selection: (position, position),
            allowed: true,
        }
    }
    #[test]
    fn suffix_backspace_preserves_prefix_and_unicode_then_retires_empty_run() {
        let mut session = InputSession::default();
        session.inserted(point(7, 1), point(10, 2), &[b'a' as u16, 0xd83d, 0xde00]);
        session.backspaced(point(10, 2), point(8, 3));
        assert_eq!(session.range(point(8, 3)), Some((7, 8)));
        assert!(session.range(point(8, 4)).is_none());
        session.inserted(point(8, 3), point(9, 4), &[b'b' as u16]);
        assert!(session
            .take(point(9, 4))
            .unwrap()
            .matches(&[b'a' as u16, b'b' as u16]));
        session.inserted(point(7, 5), point(8, 6), &[b'a' as u16]);
        session.backspaced(point(8, 6), point(7, 7));
        assert!(session.take(point(7, 7)).is_none());
    }
    #[test]
    fn backspace_rejects_split_surrogates_boundaries_and_stale_observations() {
        for change in 0..8 {
            let mut session = InputSession::default();
            session.inserted(point(7, 1), point(10, 2), &[b'a' as u16, 0xd83d, 0xde00]);
            let mut before = point(10, 2);
            let mut after = point(8, 3);
            match change {
                0 => after.selection = (9, 9), // split pair
                1 => after.selection = (6, 6),
                2 => after.selection = (10, 10),
                3 => after.context += 1,
                4 => after.generation = 2,
                5 => before.generation += 1,
                6 => after.composition_generation += 1,
                _ => after.allowed = false,
            }
            session.backspaced(before, after);
            assert!(session.take(point(10, 2)).is_none());
        }
    }
    #[test]
    fn contiguous_unicode_input_excludes_existing_prefix_and_is_consumed_once() {
        let mut session = InputSession::default();
        session.inserted(point(7, 1), point(8, 3), &[b'n' as u16]);
        session.inserted(point(8, 3), point(10, 5), &[0xd83d, 0xde00]);
        let run = session.take(point(10, 5)).unwrap();
        assert_eq!(run.range(), (7, 10));
        assert!(run.matches(&[b'n' as u16, 0xd83d, 0xde00]));
        assert!(session.take(point(10, 5)).is_none());
    }
    #[test]
    fn every_observed_boundary_prevents_reusing_old_input() {
        for change in 0..6 {
            let mut session = InputSession::default();
            session.inserted(point(0, 1), point(1, 2), &[b'a' as u16]);
            let mut current = point(1, 2);
            match change {
                0 => current.context += 1,
                1 => current.generation += 1,
                2 => current.composition_generation += 1,
                3 => current.selection = (0, 0),
                4 => current.allowed = false,
                _ => session.clear(),
            }
            assert!(session.take(current).is_none());
            assert!(session.take(point(1, 2)).is_none());
        }
    }
    #[test]
    fn replacement_or_generation_gap_starts_only_the_newly_inserted_text() {
        for mut before in [point(4, 9), point(0, 9)] {
            let mut session = InputSession::default();
            session.inserted(point(0, 1), point(3, 2), &[b'x' as u16; 3]);
            before.selection.1 += 2;
            let after = point(before.selection.0 + 1, 10);
            session.inserted(before, after, &[b'y' as u16]);
            let run = session.take(after).unwrap();
            assert_eq!(run.range(), (before.selection.0, before.selection.0 + 1));
            assert!(run.matches(&[b'y' as u16]));
        }
    }
    #[test]
    fn rejected_observations_and_malformed_input_clear_tracking() {
        for text in [
            vec![],
            vec![0xd800],
            vec![0xdc00],
            vec![0x0d],
            vec![0x1b],
            vec![b'a' as u16; MAX_TRACKED_UNITS + 1],
        ] {
            let mut session = InputSession::default();
            session.inserted(point(0, 1), point(1, 2), &[b'a' as u16]);
            session.inserted(point(1, 2), point(2, 3), &text);
            assert!(session.take(point(2, 3)).is_none());
        }
        for (before, after) in [
            (point(0, 3), point(1, 3)),
            (point(0, 3), point(1, 2)),
            (point(i32::MAX, 1), point(i32::MAX, 2)),
            (point(0, 1), point(1, u64::MAX)),
        ] {
            let mut session = InputSession::default();
            session.inserted(before, after, &[b'a' as u16]);
            assert!(session.take(after).is_none());
        }
    }
    #[test]
    fn exact_capacity_is_allowed_and_overflow_never_tracks_a_partial_suffix() {
        let mut session = InputSession::default();
        session.inserted(
            point(0, 1),
            point(1024, 2),
            &[b'a' as u16; MAX_TRACKED_UNITS],
        );
        assert_eq!(
            session.take(point(1024, 2)).unwrap().text().len(),
            MAX_TRACKED_UNITS
        );
        session.inserted(
            point(0, 3),
            point(1024, 4),
            &[b'a' as u16; MAX_TRACKED_UNITS],
        );
        session.inserted(point(1024, 4), point(1025, 5), &[b'b' as u16]);
        assert!(session.take(point(1025, 5)).is_none());
    }
}
