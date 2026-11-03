use super::*;
fn point(selection: (i32, i32), generation: u64) -> InputPoint {
    InputPoint {
        context: 1,
        generation,
        composition_generation: 0,
        selection,
        allowed: true,
    }
}
fn recorded(text: &str) -> (InputSession, InputPoint) {
    let units: Vec<_> = text.encode_utf16().collect();
    let end = point((7 + units.len() as i32, 7 + units.len() as i32), 2);
    let mut session = InputSession::default();
    session.inserted(point((7, 7), 1), end, &units);
    (session, end)
}
fn apply(
    session: &mut InputSession,
    before: InputPoint,
    direction: Direction,
    extend: bool,
    active_start: bool,
    count: usize,
) -> (InputPoint, bool) {
    let (selection, active) = session
        .selection_range(before, direction, extend, active_start, count)
        .unwrap();
    let after = point(selection, before.generation + 1);
    session.selected(before, after, direction, extend, active_start, count);
    assert!(session.range(after).is_some());
    (after, active)
}
#[test]
fn selection_preserves_the_anchor_through_shrink_collapse_and_reversal() {
    use Direction::*;
    let (mut session, end) = recorded("abcde");
    let (middle, _) = apply(&mut session, end, Backward, false, false, 2);
    let (left, active) = apply(&mut session, middle, Backward, true, false, 2);
    assert_eq!((left.selection, active), ((8, 10), true));
    let (shrunk, active) = apply(&mut session, left, Forward, true, active, 1);
    assert_eq!((shrunk.selection, active), ((9, 10), true));
    let (collapsed, active) = apply(&mut session, shrunk, Forward, true, active, 1);
    assert_eq!((collapsed.selection, active), ((10, 10), false));
    let (right, active) = apply(&mut session, collapsed, Forward, true, active, 2);
    assert_eq!((right.selection, active), ((10, 12), false));
    let (crossed, active) = apply(&mut session, right, Backward, true, active, 3);
    assert_eq!((crossed.selection, active), ((9, 10), true));
    assert!(session
        .take(crossed)
        .unwrap()
        .matches(&"abcde".encode_utf16().collect::<Vec<_>>()));
}
#[test]
fn normal_arrows_collapse_before_applying_the_remaining_repeats() {
    use Direction::*;
    for active_start in [false, true] {
        let (mut session, end) = recorded("ae\u{301}👨‍👩‍👧‍👦🇯🇵b");
        let (selected, _) = apply(&mut session, end, Backward, true, false, 3);
        for (direction, count, caret) in [(Backward, 1, 10), (Backward, 2, 8), (Forward, 1, 26)] {
            assert_eq!(
                session.selection_range(selected, direction, false, active_start, count),
                Some(((caret, caret), false))
            );
        }
        assert_eq!(
            session.selection_range(selected, Forward, false, active_start, 2),
            None
        );
    }
}
#[test]
fn selection_repeats_require_whole_admission_and_queries_do_not_consume() {
    use Direction::*;
    let (mut session, end) = recorded(&"漢".repeat(MAX_TRACKED_UNITS));
    for count in [0, 1025, usize::MAX] {
        assert_eq!(
            session.selection_range(end, Backward, true, false, count),
            None
        );
    }
    let (all, active) = apply(&mut session, end, Backward, true, false, 1024);
    assert_eq!((all.selection, active), ((7, 1031), true));
    assert_eq!(session.selection_range(all, Backward, true, true, 1), None);
    assert_eq!(session.range(all), Some((7, 1031)));
    assert_eq!(session.selection_range(all, Forward, false, true, 2), None);
}
#[test]
fn invalid_selection_completions_retire_the_private_record() {
    for after in [
        point((7, 8), 3),
        point((8, 11), 2),
        InputPoint {
            context: 2,
            ..point((8, 11), 3)
        },
        InputPoint {
            allowed: false,
            ..point((8, 11), 3)
        },
        InputPoint {
            composition_generation: 1,
            ..point((8, 11), 3)
        },
    ] {
        let (mut session, before) = recorded("a😀b");
        session.selected(before, after, Direction::Backward, true, false, 2);
        assert!(session.take(after).is_none());
    }
}
#[test]
fn a_rejoined_cluster_can_keep_a_scalar_anchor_without_splitting_surrogates() {
    use Direction::*;
    let (mut session, end) = recorded("🇯x🇵b");
    let (middle, _) = apply(&mut session, end, Backward, false, false, 3);
    session.deleted_forward(middle, point((9, 9), 4), 1);
    let (selected, active) = apply(&mut session, point((9, 9), 4), Backward, true, false, 1);
    assert_eq!((selected.selection, active), ((7, 9), true));
    assert_eq!(
        session.selection_range(selected, Forward, false, true, 1),
        Some(((11, 11), false))
    );
    assert!(session
        .take(selected)
        .unwrap()
        .matches(&"🇯🇵b".encode_utf16().collect::<Vec<_>>()));
}
#[test]
fn collapsing_a_selection_keeps_both_sides_for_later_verified_insertion() {
    use Direction::*;
    let (mut session, end) = recorded("a😀b");
    let (selected, active) = apply(&mut session, end, Backward, true, false, 2);
    let (caret, _) = apply(&mut session, selected, Backward, false, active, 1);
    let after = point((9, 9), caret.generation + 1);
    session.inserted_at_caret(caret, after, &['z' as u16]);
    let text = session.take(after).unwrap();
    assert_eq!(text.range(), (7, 12));
    assert!(text.matches(&"az😀b".encode_utf16().collect::<Vec<_>>()));
}
