//! Bounded metadata plans for a caller-owned UTF-16 snapshot. No document read,
//! edit permission, retained text, COM reference, normalization or key replay.
use crate::input::{with_utf16_text, MAX_TRACKED_UNITS};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Direction {
    Backward,
    Forward,
}

/// Offsets only, using the same pinned Unicode data as recorded Backspace.
/// Rebuild after every text change; this object never authorizes an edit.
pub struct Boundaries {
    points: [u16; MAX_TRACKED_UNITS + 1],
    count: usize,
    scalars: [u64; (MAX_TRACKED_UNITS + 1).div_ceil(64)],
}
impl Boundaries {
    pub fn new(units: &[u16]) -> Option<Self> {
        with_utf16_text(units, |text| {
            let mut map = Self {
                points: [0; MAX_TRACKED_UNITS + 1],
                count: 1,
                scalars: [0; (MAX_TRACKED_UNITS + 1).div_ceil(64)],
            };
            map.scalars[0] = 1;
            let mut offset = 0;
            for value in text.chars() {
                offset += value.len_utf16();
                map.scalars[offset / 64] |= 1u64 << (offset % 64);
            }
            offset = 0;
            for cluster in text.graphemes(true) {
                offset += cluster.encode_utf16().count();
                *map.points.get_mut(map.count)? = offset as u16;
                map.count += 1;
            }
            Some(map)
        })
    }
    pub fn end(&self) -> usize {
        self.points[self.count - 1] as usize
    }
    fn valid_caret(&self, caret: usize) -> bool {
        caret <= self.end() && self.scalars[caret / 64] & (1u64 << (caret % 64)) != 0
    }
    fn index(&self, caret: usize) -> Option<Result<usize, usize>> {
        self.valid_caret(caret)
            .then(|| self.points[..self.count].binary_search(&(caret as u16)))
    }
    /// At an interior scalar boundary, snap to the containing cluster's edge.
    /// Never accept a position inside a surrogate pair.
    pub fn snap(&self, caret: usize, direction: Direction) -> Option<usize> {
        let index = match self.index(caret)? {
            Ok(index) => index,
            Err(index) => match direction {
                Direction::Backward => index.checked_sub(1)?,
                Direction::Forward => index,
            },
        };
        Some(self.points[index] as usize)
    }
    /// One step from an interior scalar boundary reaches the containing edge.
    /// Repeats clamp at document ends and must be within the finite input cap.
    pub fn move_caret(&self, caret: usize, direction: Direction, count: usize) -> Option<usize> {
        if !(1..=MAX_TRACKED_UNITS).contains(&count) {
            return None;
        }
        let current = self.index(caret)?;
        let index = match direction {
            Direction::Backward => current.unwrap_or_else(|index| index).saturating_sub(count),
            Direction::Forward => {
                let index = match current {
                    Ok(index) => index,
                    Err(index) => index.checked_sub(1)?,
                };
                index.saturating_add(count).min(self.count - 1)
            }
        };
        Some(self.points[index] as usize)
    }
    /// A collapsed interior caret deletes the whole containing cluster first.
    /// Explicit nonempty selections retain their exact valid UTF-16 endpoints;
    /// repeat does not enlarge an explicit selection. Empty edge ranges are OK.
    pub fn deletion(
        &self,
        selection: (usize, usize),
        direction: Direction,
        count: usize,
    ) -> Option<(usize, usize)> {
        let (start, end) = selection;
        if !(1..=MAX_TRACKED_UNITS).contains(&count)
            || start > end
            || !self.valid_caret(start)
            || !self.valid_caret(end)
        {
            return None;
        }
        if start != end {
            return Some(selection);
        }
        let current = self.index(start)?;
        let (first, last) = match direction {
            Direction::Backward => {
                let last = current.unwrap_or_else(|index| index);
                (last.saturating_sub(count), last)
            }
            Direction::Forward => {
                let first = match current {
                    Ok(index) => index,
                    Err(index) => index.checked_sub(1)?,
                };
                (first, first.saturating_add(count).min(self.count - 1))
            }
        };
        Some((self.points[first] as usize, self.points[last] as usize))
    }
}

#[cfg(test)]
mod tests {
    use super::{Direction::*, *};
    fn map(text: &str) -> Boundaries {
        Boundaries::new(&text.encode_utf16().collect::<Vec<_>>()).unwrap()
    }
    #[test]
    fn empty_ascii_and_repeats_clamp_at_both_ends() {
        let empty = map("");
        assert_eq!(empty.move_caret(0, Forward, 1), Some(0));
        assert_eq!(empty.deletion((0, 0), Backward, 1), Some((0, 0)));
        let ascii = map("abc");
        assert_eq!(ascii.move_caret(1, Forward, 1024), Some(3));
        assert_eq!(ascii.move_caret(2, Backward, 1024), Some(0));
        assert_eq!(ascii.deletion((3, 3), Forward, 1), Some((3, 3)));
    }
    #[test]
    fn interior_carets_snap_and_move_to_whole_cluster_edges() {
        for cluster in ["e\u{301}", "👨‍👩‍👧‍👦", "👍🏽", "🇯🇵", "✈️"] {
            let text = format!("a{cluster}b");
            let boundaries = map(&text);
            let end = 1 + cluster.encode_utf16().count();
            let inside = if cluster.starts_with('e') || cluster.starts_with('✈') {
                2
            } else {
                3
            };
            assert_eq!(boundaries.snap(inside, Backward), Some(1));
            assert_eq!(boundaries.snap(inside, Forward), Some(end));
            assert_eq!(boundaries.move_caret(inside, Backward, 1), Some(1));
            assert_eq!(boundaries.move_caret(inside, Forward, 1), Some(end));
            assert_eq!(boundaries.move_caret(inside, Backward, 2), Some(0));
            assert_eq!(
                boundaries.move_caret(inside, Forward, 2),
                Some(boundaries.end())
            );
        }
    }
    #[test]
    fn collapsed_deletion_contains_whole_clusters_in_both_directions() {
        let boundaries = map("ae\u{301}🇯🇵b"); // [0,1,3,7,8]
        assert_eq!(boundaries.deletion((2, 2), Backward, 1), Some((1, 3)));
        assert_eq!(boundaries.deletion((2, 2), Forward, 2), Some((1, 7)));
        assert_eq!(boundaries.deletion((7, 7), Backward, 2), Some((1, 7)));
        assert_eq!(boundaries.deletion((1, 1), Forward, 2), Some((1, 7)));
        assert_eq!(boundaries.deletion((2, 2), Backward, 1024), Some((0, 3)));
    }
    #[test]
    fn explicit_partial_selections_are_not_silently_extended() {
        let boundaries = map("ae\u{301}b");
        assert_eq!(boundaries.deletion((2, 3), Forward, 10), Some((2, 3)));
        assert_eq!(boundaries.deletion((1, 2), Backward, 10), Some((1, 2)));
    }
    #[test]
    fn invalid_utf16_carets_ranges_and_counts_are_refused() {
        for units in [vec![0xd800], vec![0xdc00], vec![120; MAX_TRACKED_UNITS + 1]] {
            assert!(Boundaries::new(&units).is_none());
        }
        let boundaries = map("a😀b");
        for caret in [2, 5, usize::MAX] {
            assert_eq!(boundaries.snap(caret, Forward), None);
            assert_eq!(boundaries.move_caret(caret, Backward, 1), None);
            assert_eq!(boundaries.deletion((caret, caret), Forward, 1), None);
        }
        for count in [0, 1025, usize::MAX] {
            assert_eq!(boundaries.move_caret(0, Forward, count), None);
            assert_eq!(boundaries.deletion((0, 1), Forward, count), None);
        }
        assert_eq!(boundaries.deletion((3, 1), Forward, 1), None);
        assert_eq!(boundaries.deletion((1, 2), Forward, 1), None);
    }
    #[test]
    fn maximum_maps_and_one_maximum_cluster_fit_fixed_storage() {
        for text in ["漢".repeat(1024), format!("e{}", "\u{301}".repeat(1023))] {
            let boundaries = map(&text);
            assert_eq!(boundaries.end(), 1024);
            assert_eq!(boundaries.move_caret(0, Forward, 1024), Some(1024));
            assert_eq!(
                boundaries.deletion((1024, 1024), Backward, 1024),
                Some((0, 1024))
            );
        }
    }
    #[test]
    fn regional_indicator_parity_is_computed_from_the_whole_supplied_snapshot() {
        let boundaries = map("🇯🇵🇺🇸🇬"); // pairs plus one unmatched indicator
        for (position, next) in [(0, 4), (4, 8), (8, 10)] {
            assert_eq!(boundaries.move_caret(position, Forward, 1), Some(next));
            assert_eq!(boundaries.move_caret(next, Backward, 1), Some(position));
            assert_eq!(
                boundaries.deletion((position, position), Forward, 1),
                Some((position, next))
            );
        }
    }
}
