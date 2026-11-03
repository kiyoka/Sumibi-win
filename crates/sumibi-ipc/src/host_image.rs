//! Bounded PE32+ x64 Host metadata, not signature, dependency or launch trust.
#[path = "host_imports.rs"]
mod import_table;
pub use import_table::{ImportLibrary, ImportSymbol};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

const MAX_FILE: usize = 64 * 1024 * 1024;
const MAX_SECTIONS: usize = 96;
const MAX_IMPORTS: usize = 128;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageError {
    FileUnavailable,
    TooLarge,
    InvalidImage,
    UnsupportedArchitecture,
    NotExecutable,
    UnsupportedSubsystem,
    UnsupportedDelayImports,
    UnsupportedManagedImage,
    UnsupportedBoundImports,
    InvalidImport,
    TooManyImports,
    TooManySymbols,
    NotDll,
    InvalidExport,
    TooManyExports,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostSubsystem {
    Gui,
    Console,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostImage {
    entry_point: u32,
    subsystem: HostSubsystem,
    imports: Vec<String>,
    libraries: Vec<ImportLibrary>,
}
impl HostImage {
    pub fn entry_point_rva(&self) -> u32 {
        self.entry_point
    }
    pub fn subsystem(&self) -> HostSubsystem {
        self.subsystem
    }
    /// Canonical lowercase DLL names only. This does not resolve API sets,
    /// forwarded exports, dynamically loaded modules or the files to load.
    pub fn imports(&self) -> &[String] {
        &self.imports
    }
    /// Exact case-sensitive symbol names / ordinals from unbound static imports.
    /// These do not identify the files or exports the loader will resolve.
    pub fn import_libraries(&self) -> &[ImportLibrary] {
        &self.libraries
    }
}
pub(crate) struct Section {
    pub(crate) address: u32,
    pub(crate) virtual_size: u32,
    pub(crate) raw: u32,
    pub(crate) raw_size: u32,
    pub(crate) executable: bool,
}

pub(crate) fn inspect_file(file: &mut File) -> Result<HostImage, ImageError> {
    inspect_bytes(&read_image_file(file)?)
}

pub(crate) fn read_image_file(file: &mut File) -> Result<Vec<u8>, ImageError> {
    let size = file
        .metadata()
        .map_err(|_| ImageError::FileUnavailable)?
        .len();
    if size > MAX_FILE as u64 {
        return Err(ImageError::TooLarge);
    }
    // Exclusive access to the retained handle avoids cloned-handle cursor races.
    file.seek(SeekFrom::Start(0))
        .map_err(|_| ImageError::FileUnavailable)?;
    let mut bytes = Vec::new();
    file.take(size + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ImageError::FileUnavailable)?;
    if bytes.len() as u64 != size {
        return Err(ImageError::InvalidImage);
    }
    Ok(bytes)
}

pub(crate) struct ImageLayout<'a> {
    pub(crate) optional: &'a [u8],
    pub(crate) flags: u16,
    pub(crate) sections: Vec<Section>,
}

pub(crate) fn image_layout(bytes: &[u8]) -> Result<ImageLayout<'_>, ImageError> {
    if bytes.len() > MAX_FILE {
        return Err(ImageError::TooLarge);
    }
    let dos = region(bytes, 0, 64)?;
    if &dos[..2] != b"MZ" {
        return Err(ImageError::InvalidImage);
    }
    let pe = u32_at(dos, 0x3c)? as usize;
    if !(64..=1024 * 1024).contains(&pe) || region(bytes, pe, 4)? != b"PE\0\0" {
        return Err(ImageError::InvalidImage);
    }
    let coff = region(bytes, pe + 4, 20)?;
    if u16_at(coff, 0)? != 0x8664 {
        return Err(ImageError::UnsupportedArchitecture);
    }
    let sections_count = u16_at(coff, 2)? as usize;
    let optional_size = u16_at(coff, 16)? as usize;
    let flags = u16_at(coff, 18)?;
    if flags & 0x0002 == 0 {
        return Err(ImageError::NotExecutable);
    }
    if !(1..=MAX_SECTIONS).contains(&sections_count) || !(240..=4096).contains(&optional_size) {
        return Err(ImageError::InvalidImage);
    }
    let optional = region(bytes, pe + 24, optional_size)?;
    if u16_at(optional, 0)? != 0x20b {
        return Err(ImageError::UnsupportedArchitecture);
    }
    if u32_at(optional, 108)? != 16 {
        return Err(ImageError::InvalidImage);
    }
    let image_size = u32_at(optional, 56)?;
    let header_size = u32_at(optional, 60)?;
    if image_size == 0 || image_size > 1024 * 1024 * 1024 || header_size as usize > bytes.len() {
        return Err(ImageError::InvalidImage);
    }
    let table_start = pe + 24 + optional_size;
    let table = region(bytes, table_start, sections_count * 40)?;
    if table_start + table.len() > header_size as usize {
        return Err(ImageError::InvalidImage);
    }
    let mut sections = Vec::with_capacity(sections_count);
    for chunk in table.as_chunks::<40>().0 {
        let section = Section {
            virtual_size: u32_at(chunk, 8)?,
            address: u32_at(chunk, 12)?,
            raw_size: u32_at(chunk, 16)?,
            raw: u32_at(chunk, 20)?,
            executable: u32_at(chunk, 36)? & 0x20000000 != 0,
        };
        let virtual_end = section
            .address
            .checked_add(section.virtual_size.max(section.raw_size))
            .ok_or(ImageError::InvalidImage)?;
        if section.address < header_size || virtual_end > image_size {
            return Err(ImageError::InvalidImage);
        }
        if section.raw_size != 0 {
            if section.raw < header_size {
                return Err(ImageError::InvalidImage);
            }
            region(bytes, section.raw as usize, section.raw_size as usize)?;
        }
        for previous in &sections {
            if overlaps(previous, &section) {
                return Err(ImageError::InvalidImage);
            }
        }
        sections.push(section);
    }
    Ok(ImageLayout {
        optional,
        flags,
        sections,
    })
}

/// Inspect a complete bounded buffer. This deliberately supports one narrow
/// Host profile, not every Windows-loadable PE image, and never loads code.
pub fn inspect_bytes(bytes: &[u8]) -> Result<HostImage, ImageError> {
    let ImageLayout {
        optional,
        flags,
        sections,
    } = image_layout(bytes)?;
    if flags & 0x2000 != 0 {
        return Err(ImageError::NotExecutable);
    }
    let entry_point = u32_at(optional, 16)?;
    let subsystem = match u16_at(optional, 68)? {
        2 => HostSubsystem::Gui,
        3 => HostSubsystem::Console,
        _ => return Err(ImageError::UnsupportedSubsystem),
    };
    if entry_point == 0
        || !sections
            .iter()
            .any(|section| section.executable && rva_in(section, entry_point, 1).is_some())
    {
        return Err(ImageError::NotExecutable);
    }
    let delay_rva = u32_at(optional, 112 + 13 * 8)?;
    let delay_size = u32_at(optional, 116 + 13 * 8)?;
    if delay_rva != 0 || delay_size != 0 {
        return Err(ImageError::UnsupportedDelayImports);
    }
    if u32_at(optional, 112 + 14 * 8)? != 0 || u32_at(optional, 116 + 14 * 8)? != 0 {
        return Err(ImageError::UnsupportedManagedImage);
    }
    if u32_at(optional, 112 + 11 * 8)? != 0 || u32_at(optional, 116 + 11 * 8)? != 0 {
        return Err(ImageError::UnsupportedBoundImports);
    }
    let import_rva = u32_at(optional, 112 + 8)?;
    let import_size = u32_at(optional, 116 + 8)?;
    let libraries = import_table::inspect(bytes, &sections, import_rva, import_size)?;
    let imports = libraries
        .iter()
        .map(|library| library.name().to_owned())
        .collect();
    Ok(HostImage {
        entry_point,
        subsystem,
        imports,
        libraries,
    })
}

pub(crate) fn region(bytes: &[u8], start: usize, length: usize) -> Result<&[u8], ImageError> {
    bytes
        .get(start..start.checked_add(length).ok_or(ImageError::InvalidImage)?)
        .ok_or(ImageError::InvalidImage)
}
pub(crate) fn u16_at(bytes: &[u8], offset: usize) -> Result<u16, ImageError> {
    Ok(u16::from_le_bytes(
        region(bytes, offset, 2)?.try_into().unwrap(),
    ))
}
pub(crate) fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, ImageError> {
    Ok(u32::from_le_bytes(
        region(bytes, offset, 4)?.try_into().unwrap(),
    ))
}
fn overlaps(a: &Section, b: &Section) -> bool {
    let virtual_overlap = a.address < b.address + b.virtual_size.max(b.raw_size)
        && b.address < a.address + a.virtual_size.max(a.raw_size);
    let raw_overlap = a.raw_size != 0
        && b.raw_size != 0
        && (a.raw as u64) < b.raw as u64 + b.raw_size as u64
        && (b.raw as u64) < a.raw as u64 + a.raw_size as u64;
    virtual_overlap || raw_overlap
}
fn rva_in(section: &Section, address: u32, size: u32) -> Option<usize> {
    let offset = address.checked_sub(section.address)?;
    let end = offset.checked_add(size)?;
    if end > section.virtual_size.max(section.raw_size) || end > section.raw_size {
        return None;
    }
    section
        .raw
        .checked_add(offset)
        .map(|offset| offset as usize)
}
pub(crate) fn rva(sections: &[Section], address: u32, size: u32) -> Result<usize, ImageError> {
    sections
        .iter()
        .find_map(|section| rva_in(section, address, size))
        .ok_or(ImageError::InvalidImport)
}

pub(crate) fn image_ascii(
    bytes: &[u8],
    sections: &[Section],
    base: u32,
    max: usize,
) -> Result<String, ImageError> {
    import_table::ascii_string(bytes, sections, base, max)
}
#[cfg(test)]
mod tests {
    use super::*;

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
        put32(&mut bytes, 0x3c, 0x80);
        bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
        put16(&mut bytes, 0x84, 0x8664);
        put16(&mut bytes, 0x86, 1);
        put16(&mut bytes, 0x94, 240);
        put16(&mut bytes, 0x96, 2);
        put16(&mut bytes, OPTIONAL, 0x20b);
        put32(&mut bytes, OPTIONAL + 16, 0x1000);
        put32(&mut bytes, OPTIONAL + 56, 0x2000);
        put32(&mut bytes, OPTIONAL + 60, 0x200);
        put16(&mut bytes, OPTIONAL + 68, 3);
        put32(&mut bytes, OPTIONAL + 108, 16);
        put32(&mut bytes, OPTIONAL + 120, 0x1100);
        put32(&mut bytes, OPTIONAL + 124, 40);
        put32(&mut bytes, SECTION + 8, 0x600);
        put32(&mut bytes, SECTION + 12, 0x1000);
        put32(&mut bytes, SECTION + 16, 0x600);
        put32(&mut bytes, SECTION + 20, 0x200);
        put32(&mut bytes, SECTION + 36, 0x60000020);
        put32(&mut bytes, 0x300, 0x11c0);
        put32(&mut bytes, 0x300 + 12, 0x1180);
        put32(&mut bytes, 0x300 + 16, 0x11c0);
        bytes[0x380..0x38d].copy_from_slice(b"KERNEL32.dll\0");
        bytes
    }

    #[test]
    fn host_image_reads_console_gui_and_sorted_static_imports() {
        let mut bytes = fixture();
        let image = inspect_bytes(&bytes).unwrap();
        assert_eq!(image.entry_point_rva(), 0x1000);
        assert_eq!(image.subsystem(), HostSubsystem::Console);
        assert_eq!(image.imports(), &["kernel32.dll"]);
        put16(&mut bytes, OPTIONAL + 68, 2);
        put32(&mut bytes, OPTIONAL + 124, 60);
        put32(&mut bytes, 0x314, 0x11d0);
        put32(&mut bytes, 0x314 + 12, 0x11a0);
        put32(&mut bytes, 0x314 + 16, 0x11d0);
        bytes[0x3a0..0x3ad].copy_from_slice(b"ADVAPI32.DLL\0");
        let image = inspect_bytes(&bytes).unwrap();
        assert_eq!(image.subsystem(), HostSubsystem::Gui);
        assert_eq!(image.imports(), &["advapi32.dll", "kernel32.dll"]);
        put32(&mut bytes, OPTIONAL + 120, 0);
        put32(&mut bytes, OPTIONAL + 124, 0);
        assert!(inspect_bytes(&bytes).unwrap().imports().is_empty());
    }

    #[test]
    fn host_image_rejects_dll_other_architecture_and_hidden_dependency_forms() {
        for (offset, value, error) in [
            (0x84, 0x14c, ImageError::UnsupportedArchitecture),
            (OPTIONAL, 0x10b, ImageError::UnsupportedArchitecture),
            (0x96, 0x2002, ImageError::NotExecutable),
            (0x96, 0, ImageError::NotExecutable),
            (OPTIONAL + 68, 1, ImageError::UnsupportedSubsystem),
        ] {
            let mut bytes = fixture();
            put16(&mut bytes, offset, value);
            assert_eq!(inspect_bytes(&bytes), Err(error));
        }
        for (directory, error) in [
            (13, ImageError::UnsupportedDelayImports),
            (14, ImageError::UnsupportedManagedImage),
            (11, ImageError::UnsupportedBoundImports),
        ] {
            for field in [112, 116] {
                let mut bytes = fixture();
                put32(&mut bytes, OPTIONAL + field + directory * 8, 1);
                assert_eq!(inspect_bytes(&bytes), Err(error));
            }
        }
    }

    #[test]
    fn host_image_rejects_truncation_overflow_and_overlapping_sections() {
        let bytes = fixture();
        for length in 0..bytes.len() {
            assert!(inspect_bytes(&bytes[..length]).is_err(), "length {length}");
        }
        for (offset, value) in [
            (0x3c, u32::MAX),
            (OPTIONAL + 60, 0x180),
            (SECTION + 12, u32::MAX),
            (SECTION + 20, u32::MAX),
            (SECTION + 16, u32::MAX),
        ] {
            let mut bytes = fixture();
            put32(&mut bytes, offset, value);
            assert!(inspect_bytes(&bytes).is_err());
        }
        for raw_overlap in [false, true] {
            let mut bytes = fixture();
            put16(&mut bytes, 0x86, 2);
            let first = bytes[SECTION..SECTION + 40].to_vec();
            bytes[SECTION + 40..SECTION + 80].copy_from_slice(&first);
            if raw_overlap {
                put32(&mut bytes, OPTIONAL + 56, 0x3000);
                put32(&mut bytes, SECTION + 40 + 12, 0x2000);
            }
            assert_eq!(inspect_bytes(&bytes), Err(ImageError::InvalidImage));
        }
    }

    #[test]
    fn host_image_requires_entry_and_imports_in_file_backed_sections() {
        let mut bytes = fixture();
        put32(&mut bytes, SECTION + 8, 0x800);
        put32(&mut bytes, OPTIONAL + 16, 0x1700);
        assert_eq!(inspect_bytes(&bytes), Err(ImageError::NotExecutable));
        put32(&mut bytes, OPTIONAL + 16, 0x1000);
        put32(&mut bytes, OPTIONAL + 120, 0x1700);
        assert_eq!(inspect_bytes(&bytes), Err(ImageError::InvalidImport));
        let mut bytes = fixture();
        put32(&mut bytes, SECTION + 36, 0x40000040);
        assert_eq!(inspect_bytes(&bytes), Err(ImageError::NotExecutable));
        for address in [0, 0x100, u32::MAX] {
            let mut bytes = fixture();
            put32(&mut bytes, 0x300 + 12, address);
            assert_eq!(inspect_bytes(&bytes), Err(ImageError::InvalidImport));
        }
    }

    #[test]
    fn host_image_rejects_path_names_and_unterminated_imports() {
        for name in [
            b"../bad.dll\0".as_slice(),
            b"bad/path.dll\0",
            b"bad:ads.dll\0",
            b"no_extension\0",
        ] {
            let mut bytes = fixture();
            bytes[0x380..0x400].fill(0);
            bytes[0x380..0x380 + name.len()].copy_from_slice(name);
            assert_eq!(inspect_bytes(&bytes), Err(ImageError::InvalidImport));
        }
        let mut bytes = fixture();
        put32(&mut bytes, OPTIONAL + 124, 20);
        assert_eq!(inspect_bytes(&bytes), Err(ImageError::InvalidImport));
        let mut bytes = fixture();
        bytes[0x380..0x401].fill(b'a');
        assert_eq!(inspect_bytes(&bytes), Err(ImageError::InvalidImport));
        let mut bytes = fixture();
        put32(&mut bytes, OPTIONAL + 124, 2600);
        assert_eq!(inspect_bytes(&bytes), Err(ImageError::TooManyImports));
    }

    #[test]
    fn host_image_normalizes_repeated_dlls_but_bounds_all_descriptors() {
        let mut bytes = fixture();
        put32(&mut bytes, OPTIONAL + 124, 60);
        let descriptor = bytes[0x300..0x314].to_vec();
        bytes[0x314..0x328].copy_from_slice(&descriptor);
        put32(&mut bytes, 0x314 + 12, 0x11a0);
        bytes[0x3a0..0x3ad].copy_from_slice(b"kernel32.DLL\0");
        assert_eq!(inspect_bytes(&bytes).unwrap().imports(), &["kernel32.dll"]);

        bytes.resize(0x1200, 0);
        put32(&mut bytes, SECTION + 8, 0x1000);
        put32(&mut bytes, SECTION + 16, 0x1000);
        put32(&mut bytes, OPTIONAL + 124, 129 * 20);
        for index in 0..129 {
            let row = 0x300 + index * 20;
            bytes[row..row + 20].fill(0);
            put32(&mut bytes, row, 0x1f40);
            put32(&mut bytes, row + 12, 0x1f00);
            put32(&mut bytes, row + 16, 0x1f40);
        }
        bytes[0x1100..0x110d].copy_from_slice(b"kernel32.dll\0");
        assert_eq!(inspect_bytes(&bytes), Err(ImageError::TooManyImports));
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn host_image_reads_retained_file_from_start_on_every_inspection() {
        let path = std::env::current_exe().unwrap();
        let expected = inspect_bytes(&std::fs::read(&path).unwrap()).unwrap();
        let mut file = File::open(path).unwrap();
        file.seek(SeekFrom::End(0)).unwrap();
        assert_eq!(inspect_file(&mut file).unwrap(), expected);
        assert_eq!(inspect_file(&mut file).unwrap(), expected);
    }

    #[test]
    fn host_image_bounded_malformed_inputs_never_panic() {
        let mut seed = 0x9e3779b9u32;
        for index in 0..2048 {
            let mut bytes = fixture();
            for _ in 0..1 + index % 8 {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let position = seed as usize % bytes.len();
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                bytes[position] = (seed >> 24) as u8;
            }
            let _ = inspect_bytes(&bytes);
        }
    }

    fn put64(bytes: &mut [u8], offset: usize, value: u64) {
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
    fn symbol_fixture(values: &[u64]) -> Vec<u8> {
        assert!(values.len() <= 3);
        let mut bytes = fixture();
        put32(&mut bytes, 0x310, 0x11e0);
        for (index, value) in values.iter().enumerate() {
            put64(&mut bytes, 0x3c0 + index * 8, *value);
            put64(&mut bytes, 0x3e0 + index * 8, *value);
        }
        bytes[0x402..0x407].copy_from_slice(b"Open\0");
        bytes[0x442..0x447].copy_from_slice(b"open\0");
        bytes
    }

    #[test]
    fn host_image_collects_exact_names_and_ordinals_across_repeated_dlls() {
        let mut bytes = symbol_fixture(&[0x1200, (1u64 << 63) | 17, 0x1240]);
        put32(&mut bytes, OPTIONAL + 124, 60);
        let descriptor = bytes[0x300..0x314].to_vec();
        bytes[0x314..0x328].copy_from_slice(&descriptor);
        let image = inspect_bytes(&bytes).unwrap();
        assert_eq!(image.imports(), &["kernel32.dll"]);
        let libraries = image.import_libraries();
        assert_eq!(libraries.len(), 1);
        assert_eq!(libraries[0].name(), "kernel32.dll");
        assert_eq!(
            libraries[0].symbols(),
            &[
                ImportSymbol::Name("Open".into()),
                ImportSymbol::Name("open".into()),
                ImportSymbol::Ordinal(17),
            ]
        );
    }

    #[test]
    fn host_image_rejects_reserved_symbol_bits_and_unmapped_hint_names() {
        for invalid in [
            0x8000_0000,
            1u64 << 32,
            1u64 << 62,
            (1u64 << 63) | (1 << 16) | 17,
            (1u64 << 63) | (1 << 32) | 17,
            0x100,
            0x15ff,
        ] {
            assert_eq!(
                inspect_bytes(&symbol_fixture(&[invalid])),
                Err(ImageError::InvalidImport)
            );
        }
        let mut bytes = symbol_fixture(&[0x1200]);
        put32(&mut bytes, SECTION + 8, 0x800);
        put64(&mut bytes, 0x3c0, 0x1700);
        put64(&mut bytes, 0x3e0, 0x1700);
        assert_eq!(inspect_bytes(&bytes), Err(ImageError::InvalidImport));
    }

    #[test]
    fn host_image_requires_matching_terminated_lookup_and_address_tables() {
        let mut bytes = symbol_fixture(&[0x1200]);
        put64(&mut bytes, 0x3e0, (1u64 << 63) | 17);
        assert_eq!(inspect_bytes(&bytes), Err(ImageError::InvalidImport));
        let mut bytes = symbol_fixture(&[0x1200]);
        put64(&mut bytes, 0x3e8, (1u64 << 63) | 17);
        assert_eq!(inspect_bytes(&bytes), Err(ImageError::InvalidImport));
        for (offset, value) in [
            (0x300, 0),
            (0x310, 0),
            (0x300, u32::MAX - 3),
            (0x310, 0x15ff),
        ] {
            let mut bytes = symbol_fixture(&[0x1200]);
            put32(&mut bytes, offset, value);
            assert_eq!(inspect_bytes(&bytes), Err(ImageError::InvalidImport));
        }
        let mut bytes = symbol_fixture(&[]);
        put32(&mut bytes, 0x300, 0x15f8);
        put32(&mut bytes, 0x310, 0x15f8);
        put64(&mut bytes, 0x7f8, (1u64 << 63) | 17);
        assert_eq!(inspect_bytes(&bytes), Err(ImageError::InvalidImport));
    }

    #[test]
    fn host_image_bounds_symbol_names_and_rejects_bound_descriptor() {
        for name in [b"".as_slice(), b"has space", b"bad\x80name", b"bad\nname"] {
            let mut bytes = symbol_fixture(&[0x1200]);
            bytes[0x402..0x480].fill(0);
            bytes[0x402..0x402 + name.len()].copy_from_slice(name);
            assert_eq!(inspect_bytes(&bytes), Err(ImageError::InvalidImport));
        }
        let mut bytes = symbol_fixture(&[0x1200]);
        bytes[0x402..0x402 + 512].fill(b'a');
        bytes[0x402 + 512] = 0;
        assert!(inspect_bytes(&bytes).is_ok());
        bytes[0x402 + 512] = b'a';
        assert_eq!(inspect_bytes(&bytes), Err(ImageError::InvalidImport));
        let mut bytes = symbol_fixture(&[0x1200]);
        put32(&mut bytes, 0x304, u32::MAX);
        assert_eq!(
            inspect_bytes(&bytes),
            Err(ImageError::UnsupportedBoundImports)
        );
        let mut bytes = symbol_fixture(&[0x15ee]);
        bytes.resize(0xa00, 0);
        bytes[0x7f0..0x800].fill(b'a');
        put16(&mut bytes, 0x86, 2);
        put32(&mut bytes, SECTION + 40 + 8, 0x200);
        put32(&mut bytes, SECTION + 40 + 12, 0x1600);
        put32(&mut bytes, SECTION + 40 + 16, 0x200);
        put32(&mut bytes, SECTION + 40 + 20, 0x800);
        // A name terminated only in the next section is outside this profile.
        assert_eq!(inspect_bytes(&bytes), Err(ImageError::InvalidImport));
    }

    fn ordinal_fixture(count: usize) -> Vec<u8> {
        let mut bytes = fixture();
        bytes.resize(0x20200, 0);
        put32(&mut bytes, SECTION + 8, 0x20000);
        put32(&mut bytes, SECTION + 16, 0x20000);
        put32(&mut bytes, OPTIONAL + 56, 0x22000);
        put32(&mut bytes, 0x310, 0xb000);
        for index in 0..count {
            put64(&mut bytes, 0x3c0 + index * 8, (1u64 << 63) | 17);
            put64(&mut bytes, 0xa200 + index * 8, (1u64 << 63) | 17);
        }
        bytes
    }

    #[test]
    fn host_image_charges_duplicate_symbols_to_per_descriptor_and_global_limits() {
        let bytes = ordinal_fixture(4096);
        assert_eq!(
            inspect_bytes(&bytes).unwrap().import_libraries()[0].symbols(),
            &[ImportSymbol::Ordinal(17)]
        );
        assert_eq!(
            inspect_bytes(&ordinal_fixture(4097)),
            Err(ImageError::TooManySymbols)
        );
        let mut bytes = ordinal_fixture(3000);
        put32(&mut bytes, OPTIONAL + 124, 80);
        let descriptor = bytes[0x300..0x314].to_vec();
        bytes[0x314..0x328].copy_from_slice(&descriptor);
        bytes[0x328..0x33c].copy_from_slice(&descriptor);
        assert_eq!(inspect_bytes(&bytes), Err(ImageError::TooManySymbols));
    }
}
