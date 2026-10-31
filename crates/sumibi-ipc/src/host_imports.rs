//! Unbound static import metadata only; does not resolve dependency files.
use super::{region, rva, rva_in, u32_at, ImageError, Section, MAX_IMPORTS};
use std::collections::{BTreeMap, BTreeSet};

const MAX_SYMBOLS_PER_DESCRIPTOR: usize = 4096;
const MAX_SYMBOLS: usize = 8192;
const MAX_SYMBOL_NAME: usize = 512;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ImportSymbol {
    Name(String),
    Ordinal(u16),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportLibrary {
    name: String,
    symbols: Vec<ImportSymbol>,
}
impl ImportLibrary {
    pub fn name(&self) -> &str {
        &self.name
    }
    /// Sorted, unique symbols. Names preserve case; ordinals are not names.
    pub fn symbols(&self) -> &[ImportSymbol] {
        &self.symbols
    }
}

pub(super) fn inspect(
    bytes: &[u8],
    sections: &[Section],
    address: u32,
    size: u32,
) -> Result<Vec<ImportLibrary>, ImageError> {
    if address == 0 && size == 0 {
        return Ok(Vec::new());
    }
    if address == 0 || size < 20 {
        return Err(ImageError::InvalidImport);
    }
    if size as usize > (MAX_IMPORTS + 1) * 20 {
        return Err(ImageError::TooManyImports);
    }
    let start = rva(sections, address, size)?;
    let table = region(bytes, start, size as usize).map_err(|_| ImageError::InvalidImport)?;
    let mut libraries: BTreeMap<String, BTreeSet<ImportSymbol>> = BTreeMap::new();
    let mut total = 0;
    for (index, descriptor) in table.as_chunks::<20>().0.iter().enumerate() {
        if descriptor.iter().all(|byte| *byte == 0) {
            return Ok(libraries
                .into_iter()
                .map(|(name, symbols)| ImportLibrary {
                    name,
                    symbols: symbols.into_iter().collect(),
                })
                .collect());
        }
        if index == MAX_IMPORTS {
            return Err(ImageError::TooManyImports);
        }
        if u32_at(descriptor, 4)? != 0 {
            return Err(ImageError::UnsupportedBoundImports);
        }
        let name = ascii_string(bytes, sections, u32_at(descriptor, 12)?, 128)?;
        let name = name.to_ascii_lowercase();
        if name.len() <= 4
            || !name.ends_with(".dll")
            || name.starts_with('.')
            || name.contains("..")
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return Err(ImageError::InvalidImport);
        }
        let lookup = u32_at(descriptor, 0)?;
        let iat = u32_at(descriptor, 16)?;
        if lookup == 0 || iat == 0 {
            return Err(ImageError::InvalidImport);
        }
        // Duplicate DLL descriptors are valid; keep a single merged inventory,
        // but charge every descriptor and every symbol before deduplication.
        let symbols = libraries.entry(name).or_default();
        inspect_symbols(bytes, sections, lookup, iat, &mut total, symbols)?;
    }
    Err(ImageError::InvalidImport)
}

fn inspect_symbols(
    bytes: &[u8],
    sections: &[Section],
    lookup: u32,
    iat: u32,
    total: &mut usize,
    symbols: &mut BTreeSet<ImportSymbol>,
) -> Result<(), ImageError> {
    for index in 0..=MAX_SYMBOLS_PER_DESCRIPTOR {
        let value = thunk(bytes, sections, lookup, index)?;
        let address = thunk(bytes, sections, iat, index)?;
        if value != address {
            return Err(ImageError::InvalidImport);
        }
        if value == 0 {
            return Ok(());
        }
        if index == MAX_SYMBOLS_PER_DESCRIPTOR || *total == MAX_SYMBOLS {
            return Err(ImageError::TooManySymbols);
        }
        *total += 1;
        let symbol = if value & (1u64 << 63) != 0 {
            if value & !((1u64 << 63) | 0xffff) != 0 {
                return Err(ImageError::InvalidImport);
            }
            ImportSymbol::Ordinal(value as u16)
        } else {
            // PE32+ named imports use only bits 0..30 of the 64-bit thunk.
            if value > 0x7fff_ffff {
                return Err(ImageError::InvalidImport);
            }
            let name = value as u32;
            // The two-byte hint and name must both be file-backed. Hints are
            // advisory; export resolution must use the exact symbol name.
            rva(sections, name, 2)?;
            ImportSymbol::Name(ascii_string(
                bytes,
                sections,
                name.checked_add(2).ok_or(ImageError::InvalidImport)?,
                MAX_SYMBOL_NAME,
            )?)
        };
        symbols.insert(symbol);
    }
    Err(ImageError::TooManySymbols)
}

fn thunk(bytes: &[u8], sections: &[Section], base: u32, index: usize) -> Result<u64, ImageError> {
    let address = base
        .checked_add((index * 8) as u32)
        .ok_or(ImageError::InvalidImport)?;
    let start = rva(sections, address, 8)?;
    let value = region(bytes, start, 8).map_err(|_| ImageError::InvalidImport)?;
    Ok(u64::from_le_bytes(value.try_into().unwrap()))
}

fn ascii_string(
    bytes: &[u8],
    sections: &[Section],
    base: u32,
    max: usize,
) -> Result<String, ImageError> {
    // One bounded file-backed section per string. Resolve its RVA once, not
    // for every character (8192 repeated 512-byte names must stay cheap).
    let (start, available) = sections
        .iter()
        .find_map(|section| {
            let start = rva_in(section, base, 1)?;
            let available = section
                .raw_size
                .checked_sub(base.checked_sub(section.address)?)?;
            Some((start, available as usize))
        })
        .ok_or(ImageError::InvalidImport)?;
    let value =
        region(bytes, start, available.min(max + 1)).map_err(|_| ImageError::InvalidImport)?;
    let end = value
        .iter()
        .position(|byte| *byte == 0)
        .ok_or(ImageError::InvalidImport)?;
    if end == 0 || !value[..end].iter().all(|byte| byte.is_ascii_graphic()) {
        return Err(ImageError::InvalidImport);
    }
    String::from_utf8(value[..end].to_vec()).map_err(|_| ImageError::InvalidImport)
}
