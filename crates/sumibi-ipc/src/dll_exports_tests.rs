use super::*;
#[path = "export_resolver_tests.rs"]
mod resolver;
const OPTIONAL: usize = 0x98;
const SECTION: usize = OPTIONAL + 240;
fn put16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}
fn put32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn fixture() -> Vec<u8> {
    let mut bytes = vec![0; 0x800];
    bytes[..2].copy_from_slice(b"MZ");
    put32(&mut bytes, 60, 0x80);
    bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
    put16(&mut bytes, 0x84, 0x8664);
    put16(&mut bytes, 0x86, 1);
    put16(&mut bytes, 0x94, 240);
    put16(&mut bytes, 0x96, 0x2002);
    put16(&mut bytes, OPTIONAL, 0x20b);
    put32(&mut bytes, OPTIONAL + 56, 0x2000);
    put32(&mut bytes, OPTIONAL + 60, 0x200);
    put32(&mut bytes, OPTIONAL + 108, 16);
    put32(&mut bytes, OPTIONAL + 112, 0x1100);
    put32(&mut bytes, OPTIONAL + 116, 0x200);
    put32(&mut bytes, SECTION + 8, 0x800);
    put32(&mut bytes, SECTION + 12, 0x1000);
    put32(&mut bytes, SECTION + 16, 0x600);
    put32(&mut bytes, SECTION + 20, 0x200);
    put32(&mut bytes, SECTION + 36, 0x60000020);
    put32(&mut bytes, 0x30c, 0x1280);
    put32(&mut bytes, 0x310, 7);
    put32(&mut bytes, 0x314, 4);
    put32(&mut bytes, 0x318, 3);
    put32(&mut bytes, 0x31c, 0x1140);
    put32(&mut bytes, 0x320, 0x1180);
    put32(&mut bytes, 0x324, 0x11a0);
    for (index, value) in [0x1000, 0, 0x1240, 0x1700].into_iter().enumerate() {
        put32(&mut bytes, 0x340 + index * 4, value);
    }
    for (index, value) in [0x11c0, 0x11d0, 0x11e0].into_iter().enumerate() {
        put32(&mut bytes, 0x380 + index * 4, value);
    }
    put16(&mut bytes, 0x3a0, 0);
    put16(&mut bytes, 0x3a2, 2);
    put16(&mut bytes, 0x3a4, 0);
    bytes[0x3c0..0x3c6].copy_from_slice(b"Alias\0");
    bytes[0x3d0..0x3d8].copy_from_slice(b"Forward\0");
    bytes[0x3e0..0x3e5].copy_from_slice(b"Open\0");
    bytes[0x440..0x44d].copy_from_slice(b"other.Target\0");
    bytes[0x480..0x48c].copy_from_slice(b"fixture.dll\0");
    bytes
}
fn named(name: &str) -> ImportSymbol {
    ImportSymbol::Name(name.to_owned())
}

#[test]
fn dll_exports_lookup_preserves_aliases_holes_ordinals_and_unresolved_forwarders() {
    let exports = inspect_bytes(&fixture()).unwrap();
    assert_eq!(exports.module_name(), Some("fixture.dll"));
    assert_eq!(
        (
            exports.ordinal_base(),
            exports.slot_count(),
            exports.named_count()
        ),
        (7, 4, 3)
    );
    for symbol in [named("Alias"), named("Open"), ImportSymbol::Ordinal(7)] {
        assert_eq!(exports.lookup(&symbol), Some(&ExportTarget::Rva(0x1000)));
    }
    for symbol in [
        named("open"),
        named("Missing"),
        ImportSymbol::Ordinal(6),
        ImportSymbol::Ordinal(8),
        ImportSymbol::Ordinal(11),
    ] {
        assert_eq!(exports.lookup(&symbol), None);
    }
    assert_eq!(
        exports.lookup(&named("Forward")),
        Some(&ExportTarget::Forwarder("other.Target".into()))
    );
    assert_eq!(
        exports.lookup(&ImportSymbol::Ordinal(9)),
        exports.lookup(&named("Forward"))
    );
    assert_eq!(
        exports.lookup(&ImportSymbol::Ordinal(10)),
        Some(&ExportTarget::Rva(0x1700))
    );
    let mut bytes = fixture();
    put16(&mut bytes, 0x3a4, 1);
    assert_eq!(inspect_bytes(&bytes).unwrap().lookup(&named("Open")), None);
}

#[test]
fn dll_exports_requires_dll_profile_and_accepts_absent_exports() {
    let mut bytes = fixture();
    put16(&mut bytes, 0x96, 2);
    assert_eq!(inspect_bytes(&bytes), Err(ImageError::NotDll));
    put16(&mut bytes, 0x96, 0x2002);
    put16(&mut bytes, 0x84, 0x14c);
    assert_eq!(
        inspect_bytes(&bytes),
        Err(ImageError::UnsupportedArchitecture)
    );
    let mut bytes = fixture();
    put32(&mut bytes, OPTIONAL + 112, 0);
    put32(&mut bytes, OPTIONAL + 116, 0);
    let exports = inspect_bytes(&bytes).unwrap();
    assert_eq!((exports.slot_count(), exports.named_count()), (0, 0));
    assert_eq!(exports.module_name(), None);
    assert_eq!(exports.lookup(&named("Open")), None);
}

#[test]
fn dll_exports_rejects_truncated_overflowing_and_out_of_image_tables() {
    let bytes = fixture();
    for length in 0..bytes.len() {
        assert!(inspect_bytes(&bytes[..length]).is_err());
    }
    for (offset, value) in [
        (OPTIONAL + 112, 0),
        (OPTIONAL + 116, 39),
        (OPTIONAL + 116, u32::MAX),
        (0x300, 1),
        (0x310, u32::MAX),
        (0x31c, u32::MAX),
        (0x320, 0x1700),
        (0x324, 0),
        (0x380, 0x1700),
        (0x340, 0x1800),
        (0x340, 1),
        (0x30c, 0),
    ] {
        let mut bytes = fixture();
        put32(&mut bytes, offset, value);
        assert_eq!(
            inspect_bytes(&bytes),
            Err(ImageError::InvalidExport),
            "offset {offset:x}"
        );
    }
    for offset in [0x314, 0x318] {
        let mut bytes = fixture();
        put32(&mut bytes, offset, 65537);
        assert_eq!(inspect_bytes(&bytes), Err(ImageError::TooManyExports));
    }
}

#[test]
fn dll_exports_rejects_unsorted_duplicate_and_out_of_range_names() {
    for value in [0x11c0, 0x11e0] {
        let mut bytes = fixture();
        put32(&mut bytes, 0x384, value);
        assert_eq!(inspect_bytes(&bytes), Err(ImageError::InvalidExport));
    }
    let mut bytes = fixture();
    put16(&mut bytes, 0x3a0, 4);
    assert_eq!(inspect_bytes(&bytes), Err(ImageError::InvalidExport));
    for value in [b' ', 0x80, 0] {
        let mut bytes = fixture();
        bytes[0x3c0] = value;
        assert_eq!(inspect_bytes(&bytes), Err(ImageError::InvalidExport));
    }
}

#[test]
fn dll_exports_forwarder_terminator_stays_inside_directory_and_section() {
    let mut bytes = fixture();
    bytes[0x440..0x460].fill(0);
    bytes[0x440..0x44a].copy_from_slice(b"other.#27\0");
    assert_eq!(
        inspect_bytes(&bytes).unwrap().lookup(&named("Forward")),
        Some(&ExportTarget::Forwarder("other.#27".into()))
    );
    for value in [
        b"nodot\0".as_slice(),
        b".Target\0",
        b"other.\0",
        b"other.bad name\0",
    ] {
        let mut bytes = fixture();
        bytes[0x440..0x470].fill(0);
        bytes[0x440..0x440 + value.len()].copy_from_slice(value);
        assert_eq!(inspect_bytes(&bytes), Err(ImageError::InvalidExport));
    }
    let mut bytes = fixture();
    put32(&mut bytes, 0x348, 0x12f0);
    bytes[0x4f0..0x500].fill(b'x');
    bytes[0x4f0..0x4fd].copy_from_slice(b"other.Target\0");
    assert!(matches!(
        inspect_bytes(&bytes).unwrap().lookup(&named("Forward")),
        Some(ExportTarget::Forwarder(_))
    ));
    bytes[0x4fc] = b'x';
    assert_eq!(inspect_bytes(&bytes), Err(ImageError::InvalidExport));
    put32(&mut bytes, 0x348, 0x1300);
    assert_eq!(
        inspect_bytes(&bytes).unwrap().lookup(&named("Forward")),
        Some(&ExportTarget::Rva(0x1300))
    );
}

#[test]
fn dll_exports_supports_full_u16_ordinal_range_without_a_name_table() {
    let mut bytes = fixture();
    bytes.resize(0x42200, 0);
    put32(&mut bytes, SECTION + 8, 0x42000);
    put32(&mut bytes, SECTION + 16, 0x42000);
    put32(&mut bytes, OPTIONAL + 56, 0x44000);
    put32(&mut bytes, 0x310, 0);
    put32(&mut bytes, 0x314, 65536);
    put32(&mut bytes, 0x318, 0);
    put32(&mut bytes, 0x31c, 0x2000);
    put32(&mut bytes, 0x1200, 0x1000);
    put32(&mut bytes, 0x1200 + 65535 * 4, 0x1700);
    let exports = inspect_bytes(&bytes).unwrap();
    assert_eq!((exports.slot_count(), exports.named_count()), (65536, 0));
    assert_eq!(
        exports.lookup(&ImportSymbol::Ordinal(0)),
        Some(&ExportTarget::Rva(0x1000))
    );
    assert_eq!(
        exports.lookup(&ImportSymbol::Ordinal(65535)),
        Some(&ExportTarget::Rva(0x1700))
    );
    assert_eq!(exports.lookup(&ImportSymbol::Ordinal(1)), None);
}

#[test]
fn dll_exports_counts_repeated_forwarder_text_against_total_string_budget() {
    let mut bytes = fixture();
    bytes.resize(0x10000, 0);
    put32(&mut bytes, SECTION + 8, 0x10000);
    put32(&mut bytes, SECTION + 16, 0xfe00);
    put32(&mut bytes, OPTIONAL + 56, 0x12000);
    put32(&mut bytes, OPTIONAL + 116, 0xd000);
    put32(&mut bytes, 0x318, 0);
    put32(&mut bytes, 0x31c, 0x2000);
    // 16 bytes including NUL makes 8176 repeated 513-byte strings hit 4MiB.
    bytes[0x480..0x490].copy_from_slice(b"fixture-abc.dll\0");
    bytes[0x600..0x800].fill(b'x');
    bytes[0x600..0x606].copy_from_slice(b"other.");
    for index in 0..8177 {
        put32(&mut bytes, 0x1200 + index * 4, 0x1400);
    }
    put32(&mut bytes, 0x314, 8176);
    assert!(inspect_bytes(&bytes).is_ok());
    put32(&mut bytes, 0x314, 8177);
    assert_eq!(inspect_bytes(&bytes), Err(ImageError::TooManyExports));
}

#[test]
fn dll_exports_bounded_mutated_inputs_do_not_panic() {
    let mut seed = 0x19c6d9a3u32;
    for index in 0..2048 {
        let mut bytes = fixture();
        for _ in 0..=index % 8 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let offset = seed as usize % bytes.len();
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            bytes[offset] = (seed >> 24) as u8;
        }
        let _ = inspect_bytes(&bytes);
    }
}
