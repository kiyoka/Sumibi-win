use super::*;

fn layout() -> Layout {
    Layout::new(&spec(&"a".repeat(32))).unwrap()
}
fn remove(tree: &mut Tree, path: &str) {
    let (parent, leaf) = path.rsplit_once('\\').unwrap();
    tree.at_mut(parent).children.remove(leaf);
}
#[test]
fn full_native_schema_and_empty_hive_have_exact_resource_states() {
    let layout = layout();
    for resource in RESOURCES {
        assert_eq!(
            layout.classify(&layout.expected, resource),
            Observation::Owned
        );
        assert_eq!(
            layout.classify(&Tree::default(), resource),
            Observation::Absent
        );
    }
}
#[test]
fn different_namespace_artifacts_and_production_class_are_rejected() {
    for field in 0..5 {
        let mut identity = spec(&"a".repeat(32));
        match field {
            0 => identity.registration_target = "sumibi-registration-fixture-x64-v1".into(),
            1 => identity.dll_path = r"C:\Other.dll".into(),
            2 => identity.sha256 = "3".repeat(64),
            3 => identity.signer_thumbprint = "4".repeat(40),
            _ => identity.transaction = format!("{:032x}", sumibi_tip::CLSID_SUMIBI_TIP.to_u128()),
        }
        assert!(Layout::new(&identity).is_err());
    }
}
#[test]
fn unknown_values_and_children_anywhere_conflict_with_all_resources() {
    let layout = layout();
    let locations = [
        layout.com.clone(),
        layout.language.clone(),
        layout.categories[0].0.clone(),
        layout.categories[1].1.clone(),
    ];
    for location in locations {
        for child in [false, true] {
            let mut tree = layout.expected.clone();
            if child {
                tree.at_mut(&location)
                    .children
                    .insert("unknown".into(), Tree::default());
            } else {
                tree.at_mut(&location)
                    .values
                    .insert("unknown".into(), reg::string("preserve"));
            }
            for resource in RESOURCES {
                assert_eq!(layout.classify(&tree, resource), Observation::Conflict);
            }
        }
    }
}
#[test]
fn altered_owner_path_threading_description_enable_or_type_is_never_owned() {
    let layout = layout();
    for (path, name, value) in [
        (layout.com.clone(), OWNER, reg::string("foreign")),
        (
            format!(r"{}\inprocserver32", layout.com),
            "",
            reg::string(r"C:\other.dll"),
        ),
        (
            format!(r"{}\inprocserver32", layout.com),
            "threadingmodel",
            reg::string("Both"),
        ),
        (layout.language.clone(), "description", reg::string("Other")),
        (layout.language.clone(), "enable", reg::dword(1)),
        (layout.language.clone(), "enable", reg::string("0")),
    ] {
        let mut tree = layout.expected.clone();
        tree.at_mut(&path).values.insert(name.into(), value);
        for resource in RESOURCES {
            assert_eq!(layout.classify(&tree, resource), Observation::Conflict);
        }
    }
}
#[test]
fn one_sided_category_alias_is_incomplete_in_both_directions() {
    let layout = layout();
    for (index, resource) in [
        Resource::KeyboardCategory,
        Resource::DisplayAttributeCategory,
    ]
    .into_iter()
    .enumerate()
    {
        for missing in [&layout.categories[index].0, &layout.categories[index].1] {
            let mut tree = layout.expected.clone();
            remove(&mut tree, missing);
            assert_eq!(
                layout.classify(&tree, resource),
                Observation::OwnedIncomplete
            );
        }
    }
}
#[test]
fn partial_language_values_are_cleanup_eligible_but_not_complete() {
    let layout = layout();
    for missing in ["enable", "description"] {
        let mut tree = layout.expected.clone();
        tree.at_mut(&layout.language).values.remove(missing);
        assert_eq!(
            layout.classify(&tree, Resource::LanguageProfile),
            Observation::OwnedIncomplete
        );
    }
}
#[test]
fn interrupted_pruning_keeps_orphaned_ancestors_cleanup_eligible() {
    let layout = layout();
    for path in [r"software\classes\clsid", r"software\classes", "software"] {
        let mut tree = Tree::default();
        tree.at_mut(path);
        assert_eq!(
            layout.classify(&tree, Resource::ComServer),
            Observation::OwnedIncomplete
        );
    }
    for path in [
        r"software\microsoft\ctf\tip",
        r"software\microsoft\ctf",
        r"software\microsoft",
    ] {
        let mut tree = Tree::default();
        tree.at_mut(path);
        assert_eq!(
            layout.classify(&tree, Resource::LanguageProfile),
            Observation::OwnedIncomplete
        );
    }
}
