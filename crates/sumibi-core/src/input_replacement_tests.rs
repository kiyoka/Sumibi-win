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
fn selected(text: &str, tail: usize, count: usize) -> (InputSession, InputPoint) {
    let units: Vec<_> = text.encode_utf16().collect();
    let end = 7 + units.len() as i32;
    let mut before = point((end, end), 2);
    let mut session = InputSession::default();
    session.inserted(point((7, 7), 1), before, &units);
    if tail != 0 {
        let caret = session
            .move_range(before, Direction::Backward, tail)
            .unwrap();
        let after = point((caret, caret), 3);
        session.moved(before, after);
        before = after;
    }
    let (range, _) = session
        .selection_range(before, Direction::Backward, true, false, count)
        .unwrap();
    let after = point(range, before.generation + 1);
    session.selected(before, after, Direction::Backward, true, false, count);
    (session, after)
}
#[test]
fn selected_replacement_preserves_only_the_recorded_prefix_and_suffix() {
    for (tail, count, expected) in [(1, 1, "azb"), (0, 3, "z"), (2, 1, "z😀b"), (0, 1, "a😀z")]
    {
        let (mut session, before) = selected("a😀b", tail, count);
        let caret = before.selection.0 + 1;
        let after = point((caret, caret), before.generation + 1);
        let end = 7 + expected.encode_utf16().count() as i32;
        assert_eq!(session.replacement_range(before, 1), Some((7, end)));
        session.replaced_selection(before, after, &['z' as u16]);
        let tracked = session.take(after).unwrap();
        assert_eq!(tracked.range(), (7, end));
        assert!(tracked.matches(&expected.encode_utf16().collect::<Vec<_>>()));
    }
}
#[test]
fn selected_deletion_ignores_remaining_repeats_without_expanding_the_selection() {
    for count in [1, 3, MAX_TRACKED_UNITS] {
        for forward in [false, true] {
            let (mut session, before) = selected("a😀b", 1, 1);
            assert_eq!(session.backspace_range(before, count), Some((8, 10)));
            assert_eq!(session.forward_delete_range(before, count), Some((8, 10)));
            let after = point((8, 8), before.generation + 1);
            if forward {
                session.deleted_forward(before, after, count);
            } else {
                session.deleted_backward(before, after, count);
            }
            assert!(session
                .take(after)
                .unwrap()
                .matches(&"ab".encode_utf16().collect::<Vec<_>>()));
        }
    }
    for use_replace in [false, true] {
        let (mut session, before) = selected("a😀b", 0, 3);
        let after = point((7, 7), before.generation + 1);
        if use_replace {
            session.replaced_selection(before, after, &[]);
        } else {
            session.deleted_backward(before, after, 1024);
        }
        assert!(session.take(after).is_none());
    }
}
#[test]
fn replacement_capacity_counts_the_removed_units_before_admitting_new_text() {
    let (mut session, before) = selected(&"漢".repeat(1024), 512, 2);
    assert_eq!(session.replacement_range(before, 3), None);
    assert_eq!(session.replacement_range(before, usize::MAX), None);
    assert_eq!(session.range(before), Some((7, 1031)));
    assert_eq!(session.replacement_range(before, 2), Some((7, 1031)));
    let caret = before.selection.0 + 2;
    let after = point((caret, caret), before.generation + 1);
    session.replaced_selection(before, after, &"😀".encode_utf16().collect::<Vec<_>>());
    let tracked = session.take(after).unwrap();
    assert_eq!(tracked.range(), (7, 1031));
    assert!(tracked.matches(
        &format!("{}😀{}", "漢".repeat(510), "漢".repeat(512))
            .encode_utf16()
            .collect::<Vec<_>>()
    ));
}
#[test]
fn invalid_replacement_observations_or_supplied_text_retire_the_record() {
    for (mut after, text) in [
        (point((8, 8), 5), vec!['z' as u16]),
        (point((9, 9), 4), vec!['z' as u16]),
        (
            InputPoint {
                context: 2,
                ..point((9, 9), 5)
            },
            vec!['z' as u16],
        ),
        (
            InputPoint {
                allowed: false,
                ..point((9, 9), 5)
            },
            vec!['z' as u16],
        ),
        (
            InputPoint {
                composition_generation: 1,
                ..point((9, 9), 5)
            },
            vec!['z' as u16],
        ),
        (point((9, 9), 5), vec![0xd800]),
        (point((9, 9), 5), vec![10]),
        (point((9, 9), 5), vec![120; 1025]),
    ] {
        let (mut session, before) = selected("a😀b", 1, 1);
        if text.len() > 1 {
            after.selection = (1033, 1033);
        }
        session.replaced_selection(before, after, &text);
        assert!(session.take(after).is_none());
    }
}
#[test]
fn selection_replacement_does_not_broaden_legacy_observation_or_record_boundaries() {
    let (session, before) = selected("a😀b", 1, 1);
    for range in [(6, 10), (8, 12), (9, 10), (8, 8)] {
        assert_eq!(
            session.replacement_range(
                InputPoint {
                    selection: range,
                    ..before
                },
                1
            ),
            None
        );
    }
    assert_eq!(session.backspace_range(before, 0), None);
    assert_eq!(session.forward_delete_range(before, 1025), None);
    let (mut legacy, before) = selected("a😀b", 1, 1);
    let after = point((9, 9), before.generation + 1);
    legacy.inserted(before, after, &['z' as u16]);
    let tracked = legacy.take(after).unwrap();
    assert_eq!(tracked.range(), (8, 9));
    assert!(tracked.matches(&['z' as u16]));
    let (mut narrow, before) = selected("a😀b", 1, 1);
    narrow.inserted_at_caret(before, after, &['z' as u16]);
    assert!(narrow.take(after).is_none());
}
#[test]
fn selected_deletion_resegments_joined_text_and_keeps_partial_scalar_selections_exact() {
    let (mut session, before) = selected("🇯x🇵b", 2, 1);
    let joined = point((9, 9), before.generation + 1);
    session.replaced_selection(before, joined, &[]);
    assert_eq!(session.backspace_range(joined, 1), Some((7, 11)));
    let (range, _) = session
        .selection_range(joined, Direction::Backward, true, false, 1)
        .unwrap();
    assert_eq!(range, (7, 9));
    let partial = point(range, joined.generation + 1);
    session.selected(joined, partial, Direction::Backward, true, false, 1);
    let after = point((7, 7), partial.generation + 1);
    session.replaced_selection(partial, after, &[]);
    assert!(session
        .take(after)
        .unwrap()
        .matches(&"🇵b".encode_utf16().collect::<Vec<_>>()));
}
