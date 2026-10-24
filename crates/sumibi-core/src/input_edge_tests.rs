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
fn recorded(start: i32, text: &str) -> (InputSession, InputPoint) {
    let units: Vec<_> = text.encode_utf16().collect();
    let end = start + units.len() as i32;
    let mut session = InputSession::default();
    let before = point((end, end), 2);
    session.inserted(point((start, start), 1), before, &units);
    (session, before)
}
fn edge(document_end: i32, to_end: bool, extend: bool, active_start: bool) -> EdgeMove {
    EdgeMove {
        document_end,
        to_end,
        extend,
        active_start,
        count: 1,
    }
}
fn apply(session: &mut InputSession, before: InputPoint, edge: EdgeMove) -> (InputPoint, bool) {
    let (selection, active_start) = session.edge_range(before, edge).unwrap();
    let after = point(selection, before.generation + 1);
    session.moved_to_edge(before, after, edge);
    assert!(session.range(after).is_some());
    (after, active_start)
}
#[test]
fn document_edges_are_never_substituted_with_hard_record_boundaries() {
    let (session, before) = recorded(7, "a😀b");
    assert_eq!(
        session.edge_range(before, edge(11, false, false, false)),
        None
    );
    assert_eq!(
        session.edge_range(before, edge(11, true, false, false)),
        Some(((11, 11), false))
    );
    assert_eq!(
        session.edge_range(before, edge(18, true, false, false)),
        None
    );
    assert_eq!(session.range(before), Some((7, 11)));
    let (session, before) = recorded(0, "a😀b");
    assert_eq!(
        session.edge_range(before, edge(11, false, false, false)),
        Some(((0, 0), false))
    );
    assert_eq!(
        session.edge_range(before, edge(11, true, false, false)),
        None
    );
}
#[test]
fn edge_selection_preserves_anchor_when_reversing_and_plain_edges_collapse() {
    let (mut session, end) = recorded(0, "a😀b");
    let before = point((1, 1), 3);
    session.moved(end, before);
    let (right, active) = apply(&mut session, before, edge(4, true, true, false));
    assert_eq!((right.selection, active), ((1, 4), false));
    let (left, active) = apply(&mut session, right, edge(4, false, true, active));
    assert_eq!((left.selection, active), ((0, 1), true));
    let (right, active) = apply(&mut session, left, edge(4, true, true, active));
    assert_eq!((right.selection, active), ((1, 4), false));
    let (home, active) = apply(&mut session, right, edge(4, false, false, active));
    assert_eq!((home.selection, active), ((0, 0), false));
    let (end, active) = apply(&mut session, home, edge(4, true, false, active));
    assert_eq!((end.selection, active), ((4, 4), false));
    assert!(session
        .take(end)
        .unwrap()
        .matches(&"a😀b".encode_utf16().collect::<Vec<_>>()));
}
#[test]
fn repeats_and_noop_edges_keep_the_whole_record_without_extra_steps() {
    let (mut session, before) = recorded(0, "ab");
    let repeated = EdgeMove {
        count: MAX_TRACKED_UNITS,
        ..edge(2, true, true, false)
    };
    let (same, active) = apply(&mut session, before, repeated);
    assert_eq!((same.selection, active), ((2, 2), false));
    let (home, _) = apply(
        &mut session,
        same,
        EdgeMove {
            to_end: false,
            ..repeated
        },
    );
    assert_eq!(home.selection, (0, 2));
    let (still_home, active) = apply(
        &mut session,
        home,
        EdgeMove {
            active_start: true,
            to_end: false,
            ..repeated
        },
    );
    assert_eq!((still_home.selection, active), ((0, 2), true));
    assert_eq!(session.range(still_home), Some((0, 2)));
}
#[test]
fn invalid_counts_and_document_extents_do_not_consume_even_maximum_records() {
    let (mut session, before) = recorded(0, &"漢".repeat(1024));
    for count in [0, 1025, usize::MAX] {
        assert_eq!(
            session.edge_range(
                before,
                EdgeMove {
                    count,
                    ..edge(1024, false, true, false)
                }
            ),
            None
        );
    }
    for end in [-1, 0, 1023] {
        assert_eq!(
            session.edge_range(before, edge(end, false, true, false)),
            None
        );
    }
    assert_eq!(
        session.edge_range(before, edge(i32::MAX, true, true, false)),
        None
    );
    assert_eq!(
        session.edge_range(before, edge(i32::MAX, false, true, false)),
        Some(((0, 1024), true))
    );
    let (all, _) = apply(&mut session, before, edge(1024, false, true, false));
    assert_eq!(session.take(all).unwrap().range(), (0, 1024));
}
#[test]
fn invalid_edge_completions_retire_instead_of_reusing_the_record() {
    for after in [
        point((0, 2), 3),
        point((0, 0), 2),
        InputPoint {
            context: 2,
            ..point((0, 0), 3)
        },
        InputPoint {
            allowed: false,
            ..point((0, 0), 3)
        },
        InputPoint {
            composition_generation: 1,
            ..point((0, 0), 3)
        },
    ] {
        let (mut session, before) = recorded(0, "ab");
        session.moved_to_edge(before, after, edge(2, false, false, false));
        assert!(session.take(after).is_none());
    }
}
#[test]
fn edge_selection_keeps_a_rejoined_scalar_anchor_and_later_editing_live() {
    let (mut session, end) = recorded(0, "🇯x🇵b");
    session.moved(end, point((2, 2), 3));
    session.deleted_forward(point((2, 2), 3), point((2, 2), 4), 1);
    let (selected, active) = apply(&mut session, point((2, 2), 4), edge(5, false, true, false));
    assert_eq!((selected.selection, active), ((0, 2), true));
    session.replaced_selection(selected, point((1, 1), 6), &['z' as u16]);
    let (end, _) = apply(&mut session, point((1, 1), 6), edge(4, true, false, false));
    assert!(session
        .take(end)
        .unwrap()
        .matches(&"z🇵b".encode_utf16().collect::<Vec<_>>()));
}
