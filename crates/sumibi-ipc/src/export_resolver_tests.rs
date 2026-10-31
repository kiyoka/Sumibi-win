use super::{fixture, named, put32};
use crate::dll_exports::{inspect_bytes, DllExports};
use crate::export_resolver::{ExportCatalog, ResolveError};
use crate::host_image::ImportSymbol;

fn direct() -> DllExports {
    let mut bytes = fixture();
    put32(&mut bytes, 0x348, 0x1000);
    inspect_bytes(&bytes).unwrap()
}
fn forwarded(target: &str) -> DllExports {
    assert!(target.len() < 64);
    let mut bytes = fixture();
    bytes[0x440..0x480].fill(0);
    bytes[0x440..0x440 + target.len()].copy_from_slice(target.as_bytes());
    inspect_bytes(&bytes).unwrap()
}

#[test]
fn resolves_name_and_ordinal_chains_preserving_exact_symbols() {
    let first = forwarded("MIDDLE.#00009");
    let middle = forwarded("leaf.Open");
    let leaf = direct();
    let mut catalog = ExportCatalog::new();
    catalog.insert("First.DLL", &first).unwrap();
    catalog.insert("middle", &middle).unwrap();
    catalog.insert("LEAF.dll", &leaf).unwrap();
    let result = catalog.resolve("FIRST", &named("Forward")).unwrap();
    assert_eq!(result.rva(), 0x1000);
    assert_eq!(result.forwarder_count(), 2);
    assert_eq!(
        result
            .steps()
            .iter()
            .map(|step| step.module())
            .collect::<Vec<_>>(),
        ["first.dll", "middle.dll", "leaf.dll"]
    );
    assert_eq!(result.steps()[1].symbol(), &ImportSymbol::Ordinal(9));
    assert_eq!(result.steps()[2].symbol(), &named("Open"));
    assert_eq!(
        catalog
            .resolve("leaf", &ImportSymbol::Ordinal(7))
            .unwrap()
            .forwarder_count(),
        0
    );
    assert_eq!(
        catalog.resolve("leaf", &named("open")),
        Err(ResolveError::SymbolMissing)
    );
}

#[test]
fn detects_self_cross_module_and_alias_ordinal_cycles() {
    let single = forwarded("SELF.#9");
    let mut catalog = ExportCatalog::new();
    catalog.insert("self", &single).unwrap();
    assert_eq!(
        catalog.resolve("self.dll", &named("Forward")),
        Err(ResolveError::Cycle)
    );
    let first = forwarded("SECOND.Forward");
    let second = forwarded("First.#9");
    let mut catalog = ExportCatalog::new();
    catalog.insert("first", &first).unwrap();
    catalog.insert("second", &second).unwrap();
    assert_eq!(
        catalog.resolve("first", &named("Forward")),
        Err(ResolveError::Cycle)
    );
}

#[test]
fn rejects_ambiguous_path_and_invalid_ordinal_forwarders() {
    for target in [
        "bad/path.Open",
        "C:evil.Open",
        "bad\\path.Open",
        "other.dll.Open",
        "other.Open.More",
        "other.#",
        "other.#-1",
        "other.#+1",
        "other.#65536",
        "other.#000000",
    ] {
        let exports = forwarded(target);
        let mut catalog = ExportCatalog::new();
        catalog.insert("first", &exports).unwrap();
        assert_eq!(
            catalog.resolve("first", &named("Forward")),
            Err(ResolveError::InvalidForwarder),
            "{target}"
        );
    }
}

#[test]
fn api_sets_cannot_be_substituted_with_physical_dll_metadata() {
    let exports = direct();
    let mut catalog = ExportCatalog::new();
    for contract in [
        "api-ms-win-core-synch-l1-2-0.dll",
        "EXT-MS-win-sample-l1-1-0",
        "api-win-core-sample~AdvancedOperations.dll",
    ] {
        assert_eq!(
            catalog.insert(contract, &exports),
            Err(ResolveError::ApiSetUnresolved)
        );
        assert_eq!(
            catalog.resolve(contract, &named("Open")),
            Err(ResolveError::ApiSetUnresolved)
        );
    }
    let exports = forwarded("API-MS-win-core-synch-l1-2-0.Open");
    catalog.insert("first", &exports).unwrap();
    assert_eq!(
        catalog.resolve("first", &named("Forward")),
        Err(ResolveError::ApiSetUnresolved)
    );
}

#[test]
fn missing_modules_symbols_and_invalid_initial_identifiers_are_distinct() {
    let exports = forwarded("absent.Forward");
    let mut catalog = ExportCatalog::new();
    catalog.insert("first", &exports).unwrap();
    assert_eq!(
        catalog.resolve("first", &named("Forward")),
        Err(ResolveError::ModuleMissing)
    );
    assert_eq!(
        catalog.resolve("first", &named("Missing")),
        Err(ResolveError::SymbolMissing)
    );
    assert_eq!(
        catalog.resolve("first", &named("")),
        Err(ResolveError::InvalidSymbol)
    );
    assert_eq!(
        catalog.resolve("first", &named("bad\0name")),
        Err(ResolveError::InvalidSymbol)
    );
    assert_eq!(
        catalog.resolve("first", &named(&"x".repeat(512))),
        Err(ResolveError::SymbolMissing)
    );
    assert_eq!(
        catalog.resolve("first", &named(&"x".repeat(513))),
        Err(ResolveError::InvalidSymbol)
    );
    for module in [
        "",
        "..",
        "C:\\first.dll",
        "../first.dll",
        "first.exe",
        "first.dll.dll",
        "bad\0name",
        "bad name",
    ] {
        assert_eq!(
            catalog.insert(module, &exports),
            Err(ResolveError::InvalidModule)
        );
    }
}

#[test]
fn exactly_sixteen_forwarders_are_accepted_and_longer_chains_stop() {
    for length in [17, 18] {
        let exports: Vec<_> = (0..length)
            .map(|index| {
                if index + 1 == length {
                    direct()
                } else {
                    forwarded(&format!("node{}.Forward", index + 1))
                }
            })
            .collect();
        let mut catalog = ExportCatalog::new();
        for (index, exports) in exports.iter().enumerate() {
            catalog.insert(&format!("node{index}"), exports).unwrap();
        }
        let result = catalog.resolve("node0", &named("Forward"));
        if length == 17 {
            assert_eq!(result.unwrap().forwarder_count(), 16);
        } else {
            assert_eq!(result, Err(ResolveError::DepthExceeded));
        }
    }
}

#[test]
fn catalog_capacity_duplicate_names_and_module_length_are_bounded() {
    let exports = direct();
    let mut catalog = ExportCatalog::new();
    let boundary = format!("{}.dll", "x".repeat(124));
    catalog.insert(&boundary, &exports).unwrap();
    assert!(catalog.resolve(&boundary, &named("Open")).is_ok());
    assert_eq!(
        catalog.insert(&format!("{}.dll", "x".repeat(125)), &exports),
        Err(ResolveError::InvalidModule)
    );
    for index in 0..127 {
        catalog.insert(&format!("module{index}"), &exports).unwrap();
    }
    assert_eq!(
        catalog.insert("overflow", &exports),
        Err(ResolveError::Capacity)
    );
    assert_eq!(
        catalog.insert("MODULE0.DLL", &exports),
        Err(ResolveError::DuplicateModule)
    );
    assert!(catalog.resolve("module0", &named("Open")).is_ok());
}
