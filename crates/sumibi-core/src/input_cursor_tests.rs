use super::*;
fn point(caret: i32, generation: u64) -> InputPoint {
    InputPoint {
        context: 1,
        generation,
        composition_generation: 0,
        selection: (caret, caret),
        allowed: true,
    }
}
fn recorded(text: &str) -> (InputSession, InputPoint) {
    let units: Vec<_> = text.encode_utf16().collect();
    let end = point(7 + units.len() as i32, 2);
    let mut session = InputSession::default();
    session.inserted(point(7, 1), end, &units);
    (session, end)
}
#[test]
fn caret_changes_preserve_the_whole_record_for_conversion() {
    let (mut session, end) = recorded("ae\u{301}👨‍👩‍👧‍👦🇯🇵b");
    assert_eq!(session.move_range(end, Direction::Backward, 3), Some(10));
    let middle = point(10, 3);
    session.moved(end, middle);
    assert_eq!(session.range(middle), Some((7, 26)));
    assert_eq!(session.backspace_range(middle, 1), Some((8, 10)));
    let tracked = session.take(middle).unwrap();
    assert_eq!(tracked.range(), (7, 26));
    assert!(tracked.matches(&"ae\u{301}👨‍👩‍👧‍👦🇯🇵b".encode_utf16().collect::<Vec<_>>()));
}
#[test]
fn movement_and_deletion_refuse_whole_oversized_repeats_without_consuming() {
    let (mut session, end) = recorded("a😀b");
    for count in [0, 4, MAX_TRACKED_UNITS + 1, usize::MAX] {
        assert_eq!(session.move_range(end, Direction::Backward, count), None);
    }
    assert_eq!(session.forward_delete_range(end, 1), None);
    session.moved(end, point(7, 3));
    for count in [0, 4, MAX_TRACKED_UNITS + 1] {
        assert_eq!(session.forward_delete_range(point(7, 3), count), None);
    }
    assert_eq!(session.range(point(7, 3)), Some((7, 11)));
}
#[test]
fn forward_repeat_deletes_recorded_graphemes_and_keeps_both_sides() {
    let (mut session, end) = recorded("ae\u{301}👨‍👩‍👧‍👦🇯🇵b");
    let middle = point(10, 3);
    session.moved(end, middle);
    assert_eq!(session.forward_delete_range(middle, 2), Some((10, 25)));
    session.deleted_forward(middle, point(10, 4), 2);
    assert_eq!(session.range(point(10, 4)), Some((7, 11)));
    assert!(session
        .take(point(10, 4))
        .unwrap()
        .matches(&"ae\u{301}b".encode_utf16().collect::<Vec<_>>()));
}
#[test]
fn full_forward_delete_retires_empty_input() {
    let (mut session, end) = recorded("a😀b");
    session.moved(end, point(7, 3));
    session.deleted_forward(point(7, 3), point(7, 4), 3);
    assert!(session.take(point(7, 4)).is_none());
}
#[test]
fn invalid_moves_retire_the_record_instead_of_adopting_another_field() {
    for after in [
        point(6, 3),
        point(12, 3),
        point(9, 3),
        point(7, 2),
        InputPoint {
            context: 2,
            ..point(7, 3)
        },
        InputPoint {
            allowed: false,
            ..point(7, 3)
        },
        InputPoint {
            composition_generation: 1,
            ..point(7, 3)
        },
        InputPoint {
            selection: (7, 8),
            ..point(7, 3)
        },
    ] {
        let (mut session, end) = recorded("a😀b");
        session.moved(end, after);
        assert!(session.take(after).is_none());
    }
}
#[test]
fn typing_inside_a_run_starts_only_a_new_supplied_record() {
    let (mut session, end) = recorded("a😀b");
    let middle = point(8, 3);
    session.moved(end, middle);
    session.inserted(middle, point(9, 4), &['z' as u16]);
    let tracked = session.take(point(9, 4)).unwrap();
    assert_eq!(tracked.range(), (8, 9));
    assert!(tracked.matches(&['z' as u16]));
}
#[test]
fn failed_forward_observations_retire_the_record() {
    for (after, count) in [
        (point(8, 4), 1),
        (point(7, 3), 1),
        (point(7, 4), 4),
        (
            InputPoint {
                context: 2,
                ..point(7, 4)
            },
            1,
        ),
    ] {
        let (mut session, end) = recorded("a😀b");
        session.moved(end, point(7, 3));
        session.deleted_forward(point(7, 3), after, count);
        assert!(session.take(after).is_none());
    }
}
#[test]
fn joined_indicators_are_resegmented_before_the_next_operation() {
    let (mut session, end) = recorded("🇯x🇵b");
    session.moved(end, point(9, 3));
    session.deleted_forward(point(9, 3), point(9, 4), 1);
    assert_eq!(
        session.move_range(point(9, 4), Direction::Forward, 1),
        Some(11)
    );
    assert_eq!(session.forward_delete_range(point(9, 4), 1), Some((7, 11)));
    session.deleted_forward(point(9, 4), point(7, 5), 1);
    assert!(session.take(point(7, 5)).unwrap().matches(&['b' as u16]));
}

#[test]
fn internal_backward_deletion_keeps_the_prefix_and_suffix_recorded() {
    for (count, start, expected) in [(1, 10, "ae\u{301}🇯🇵b"), (2, 8, "a🇯🇵b"), (3, 7, "🇯🇵b")]
    {
        let (mut session, end) = recorded("ae\u{301}👨‍👩‍👧‍👦🇯🇵b");
        let before = point(21, 3);
        session.moved(end, before);
        assert_eq!(session.backspace_range(before, count), Some((start, 21)));
        let after = point(start, 4);
        session.deleted_backward(before, after, count);
        let input = session.take(after).unwrap();
        assert_eq!(
            input.range(),
            (7, 7 + expected.encode_utf16().count() as i32)
        );
        assert!(input.matches(&expected.encode_utf16().collect::<Vec<_>>()));
    }
}
#[test]
fn backward_repeats_never_cross_the_record_start_or_consume_a_query() {
    let (mut session, end) = recorded("a😀b");
    session.moved(end, point(10, 3));
    for count in [0, 3, MAX_TRACKED_UNITS + 1, usize::MAX] {
        assert_eq!(session.backspace_range(point(10, 3), count), None);
    }
    assert_eq!(session.range(point(10, 3)), Some((7, 11)));
    session.moved(point(10, 3), point(7, 4));
    assert_eq!(session.backspace_range(point(7, 4), 1), None);
    assert!(session.take(point(7, 4)).is_some());
}
#[test]
fn backward_deletion_from_a_rejoined_cluster_includes_the_whole_cluster() {
    let (mut session, end) = recorded("🇯x🇵b");
    session.moved(end, point(9, 3));
    session.deleted_forward(point(9, 3), point(9, 4), 1);
    assert_eq!(session.backspace_range(point(9, 4), 1), Some((7, 11)));
    session.deleted_backward(point(9, 4), point(7, 5), 1);
    assert!(session.take(point(7, 5)).unwrap().matches(&['b' as u16]));
}
#[test]
fn invalid_backward_completion_retires_instead_of_preserving_stale_text() {
    for (after, count) in [
        (point(9, 4), 1),
        (point(8, 3), 1),
        (point(8, 4), 3),
        (
            InputPoint {
                context: 2,
                ..point(8, 4)
            },
            1,
        ),
        (
            InputPoint {
                allowed: false,
                ..point(8, 4)
            },
            1,
        ),
    ] {
        let (mut session, end) = recorded("a😀b");
        session.moved(end, point(10, 3));
        session.deleted_backward(point(10, 3), after, count);
        assert!(session.take(after).is_none());
    }
}
#[test]
fn maximum_record_backward_deletion_preserves_the_unmodified_half() {
    let (mut session, end) = recorded(&"漢".repeat(MAX_TRACKED_UNITS));
    let before = point(519, 3);
    session.moved(end, before);
    assert_eq!(session.backspace_range(before, 513), None);
    assert_eq!(session.backspace_range(before, 512), Some((7, 519)));
    session.deleted_backward(before, point(7, 4), 512);
    let tracked = session.take(point(7, 4)).unwrap();
    assert_eq!(tracked.range(), (7, 519));
    assert!(tracked.matches(&"漢".repeat(512).encode_utf16().collect::<Vec<_>>()));
}

#[test]
fn verified_insertion_keeps_recorded_text_on_both_sides_and_at_edges() {
    for (caret, count, expected) in [(7, 3, "za😀b"), (8, 2, "az😀b"), (11, 0, "a😀bz")] {
        let (mut session, mut before) = recorded("a😀b");
        if count != 0 {
            let next = point(caret, 3);
            session.moved(before, next);
            before = next;
        }
        let after = point(caret + 1, before.generation + 1);
        session.inserted_at_caret(before, after, &['z' as u16]);
        let tracked = session.take(after).unwrap();
        assert_eq!(tracked.range(), (7, 12));
        assert!(tracked.matches(&expected.encode_utf16().collect::<Vec<_>>()));
    }
}
#[test]
fn inserted_combining_text_rebuilds_clusters_before_the_next_deletion() {
    let (mut session, end) = recorded("ab");
    session.moved(end, point(8, 3));
    session.inserted_at_caret(point(8, 3), point(9, 4), &[0x301]);
    assert_eq!(session.backspace_range(point(9, 4), 1), Some((7, 9)));
    session.deleted_backward(point(9, 4), point(7, 5), 1);
    assert!(session.take(point(7, 5)).unwrap().matches(&['b' as u16]));
}
#[test]
fn insertion_can_split_a_cluster_joined_by_a_previous_verified_deletion() {
    let (mut session, end) = recorded("🇯x🇵b");
    session.moved(end, point(9, 3));
    session.deleted_forward(point(9, 3), point(9, 4), 1);
    session.inserted_at_caret(point(9, 4), point(10, 5), &['x' as u16]);
    let tracked = session.take(point(10, 5)).unwrap();
    assert_eq!(tracked.range(), (7, 13));
    assert!(tracked.matches(&"🇯x🇵b".encode_utf16().collect::<Vec<_>>()));
}
#[test]
fn verified_insertion_rejects_stale_or_noncollapsed_observations() {
    for (before, after) in [
        (point(8, 2), point(9, 4)),
        (point(8, 3), point(10, 4)),
        (point(8, 3), point(9, 3)),
        (
            point(8, 3),
            InputPoint {
                context: 2,
                ..point(9, 4)
            },
        ),
        (
            point(8, 3),
            InputPoint {
                allowed: false,
                ..point(9, 4)
            },
        ),
        (
            point(8, 3),
            InputPoint {
                composition_generation: 1,
                ..point(9, 4)
            },
        ),
        (
            InputPoint {
                selection: (8, 10),
                ..point(8, 3)
            },
            point(9, 4),
        ),
    ] {
        let (mut session, end) = recorded("a😀b");
        session.moved(end, point(8, 3));
        session.inserted_at_caret(before, after, &['z' as u16]);
        assert!(session.take(after).is_none());
    }
    let mut empty = InputSession::default();
    empty.inserted_at_caret(point(8, 3), point(9, 4), &['z' as u16]);
    assert!(empty.take(point(9, 4)).is_none());
}
#[test]
fn verified_insertion_refuses_invalid_supplied_utf16_without_adopting_it() {
    for text in [
        vec![],
        vec![0xd800],
        vec![0xdc00],
        vec![0],
        vec![10],
        vec![120; 1025],
    ] {
        let (mut session, end) = recorded("ab");
        session.moved(end, point(8, 3));
        let after = point(8 + text.len() as i32, 4);
        session.inserted_at_caret(point(8, 3), after, &text);
        assert!(session.take(after).is_none());
    }
}
#[test]
fn verified_insertion_admits_the_full_combined_capacity_only() {
    for length in [1023, 1024] {
        let (mut session, end) = recorded(&"漢".repeat(length));
        session.moved(end, point(519, 3));
        session.inserted_at_caret(
            point(519, 3),
            point(521, 4),
            &"😀".encode_utf16().collect::<Vec<_>>(),
        );
        assert!(session.take(point(521, 4)).is_none());
    }
    let (mut session, end) = recorded(&"漢".repeat(1022));
    session.moved(end, point(519, 3));
    session.inserted_at_caret(
        point(519, 3),
        point(521, 4),
        &"😀".encode_utf16().collect::<Vec<_>>(),
    );
    let tracked = session.take(point(521, 4)).unwrap();
    assert_eq!(tracked.range(), (7, 1031));
    assert!(tracked.matches(
        &format!("{}😀{}", "漢".repeat(512), "漢".repeat(510))
            .encode_utf16()
            .collect::<Vec<_>>()
    ));
}
