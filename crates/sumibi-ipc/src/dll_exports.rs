//! Bounded x64 DLL export metadata. No loading, forwarding or trust decision.
pub use crate::host_image::ImageError;
use crate::host_image::{
    image_ascii, image_layout, read_image_file, region, rva, u16_at, u32_at, ImageLayout,
    ImportSymbol, Section,
};
use std::collections::BTreeMap;
use std::fs::File;

const MAX_EXPORTS: usize = 65536;
const MAX_STRING_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportTarget {
    /// Image-relative metadata, not a callable pointer or proof of code.
    Rva(u32),
    /// Opaque DLL.symbol / DLL.#ordinal text. Still requires resolution.
    Forwarder(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DllExports {
    module_name: Option<String>,
    ordinal_base: u32,
    slots: Vec<Option<ExportTarget>>,
    names: BTreeMap<String, usize>,
}
impl DllExports {
    /// The DLL's own untrusted export-directory name, not its file identity.
    pub fn module_name(&self) -> Option<&str> {
        self.module_name.as_deref()
    }
    pub fn ordinal_base(&self) -> u32 {
        self.ordinal_base
    }
    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }
    pub fn named_count(&self) -> usize {
        self.names.len()
    }
    /// Exact case-sensitive lookup. Zero-RVA holes are absent. The caller
    /// must separately bind this metadata to the intended retained DLL file.
    pub fn lookup(&self, symbol: &ImportSymbol) -> Option<&ExportTarget> {
        let index = match symbol {
            ImportSymbol::Name(name) => *self.names.get(name)?,
            ImportSymbol::Ordinal(ordinal) => {
                u32::from(*ordinal).checked_sub(self.ordinal_base)? as usize
            }
        };
        self.slots.get(index)?.as_ref()
    }
}

/// Read the caller's handle from its beginning, without retaining or trusting
/// the file. Keep the handle leased separately when using the metadata.
pub fn inspect_file(file: &mut File) -> Result<DllExports, ImageError> {
    inspect_bytes(&read_image_file(file)?)
}

pub fn inspect_bytes(bytes: &[u8]) -> Result<DllExports, ImageError> {
    let image = image_layout(bytes)?;
    if image.flags & 0x2000 == 0 {
        return Err(ImageError::NotDll);
    }
    parse_exports(bytes, &image).map_err(|error| match error {
        ImageError::TooManyExports => error,
        _ => ImageError::InvalidExport,
    })
}

fn parse_exports(bytes: &[u8], image: &ImageLayout<'_>) -> Result<DllExports, ImageError> {
    let address = u32_at(image.optional, 112)?;
    let size = u32_at(image.optional, 116)?;
    let mut exports = DllExports {
        module_name: None,
        ordinal_base: 0,
        slots: Vec::new(),
        names: BTreeMap::new(),
    };
    if address == 0 && size == 0 {
        return Ok(exports);
    }
    if address == 0 || size < 40 {
        return Err(ImageError::InvalidExport);
    }
    let end = address.checked_add(size).ok_or(ImageError::InvalidExport)?;
    let start = rva(&image.sections, address, size)?;
    let directory = region(bytes, start, 40)?;
    if u32_at(directory, 0)? != 0 {
        return Err(ImageError::InvalidExport);
    }
    let slots = u32_at(directory, 20)? as usize;
    let names = u32_at(directory, 24)? as usize;
    if slots > MAX_EXPORTS || names > MAX_EXPORTS {
        return Err(ImageError::TooManyExports);
    }
    if slots == 0 && names != 0 {
        return Err(ImageError::InvalidExport);
    }
    exports.ordinal_base = u32_at(directory, 16)?;
    if slots != 0 {
        exports
            .ordinal_base
            .checked_add(slots as u32 - 1)
            .ok_or(ImageError::InvalidExport)?;
    }
    let mut budget = 0;
    exports.module_name = Some(read_ascii(
        bytes,
        &image.sections,
        u32_at(directory, 12)?,
        128,
        &mut budget,
    )?);
    let addresses = table(bytes, &image.sections, u32_at(directory, 28)?, slots, 4)?;
    let pointers = table(bytes, &image.sections, u32_at(directory, 32)?, names, 4)?;
    let ordinals = table(bytes, &image.sections, u32_at(directory, 36)?, names, 2)?;
    for entry in addresses.as_chunks::<4>().0 {
        let target = u32::from_le_bytes(*entry);
        exports.slots.push(if target == 0 {
            None
        } else if (address..end).contains(&target) {
            let max = 512usize.min((end - target - 1) as usize);
            let value = read_ascii(bytes, &image.sections, target, max, &mut budget)?;
            let (module, symbol) = value.split_once('.').ok_or(ImageError::InvalidExport)?;
            if module.is_empty() || symbol.is_empty() {
                return Err(ImageError::InvalidExport);
            }
            Some(ExportTarget::Forwarder(value))
        } else {
            // Exports may be data, including zero-fill storage. Require a mapped
            // section, without asserting an executable or file-backed address.
            if !image.sections.iter().any(|section| {
                target >= section.address
                    && target < section.address + section.virtual_size.max(section.raw_size)
            }) {
                return Err(ImageError::InvalidExport);
            }
            Some(ExportTarget::Rva(target))
        });
    }
    let mut previous: Option<String> = None;
    for index in 0..names {
        let name = read_ascii(
            bytes,
            &image.sections,
            u32_at(pointers, index * 4)?,
            512,
            &mut budget,
        )?;
        if previous.as_ref().is_some_and(|previous| previous >= &name) {
            return Err(ImageError::InvalidExport);
        }
        let ordinal = u16_at(ordinals, index * 2)? as usize;
        if ordinal >= slots {
            return Err(ImageError::InvalidExport);
        }
        previous = Some(name.clone());
        exports.names.insert(name, ordinal);
    }
    Ok(exports)
}

fn table<'a>(
    bytes: &'a [u8],
    sections: &[Section],
    address: u32,
    count: usize,
    width: usize,
) -> Result<&'a [u8], ImageError> {
    if count == 0 {
        return Ok(&[]);
    }
    if address == 0 {
        return Err(ImageError::InvalidExport);
    }
    let length = count.checked_mul(width).ok_or(ImageError::InvalidExport)?;
    let start = rva(sections, address, length as u32)?;
    region(bytes, start, length)
}

fn read_ascii(
    bytes: &[u8],
    sections: &[Section],
    address: u32,
    max: usize,
    budget: &mut usize,
) -> Result<String, ImageError> {
    let value = image_ascii(bytes, sections, address, max)?;
    *budget = budget
        .checked_add(value.len() + 1)
        .ok_or(ImageError::TooManyExports)?;
    if *budget > MAX_STRING_BYTES {
        return Err(ImageError::TooManyExports);
    }
    Ok(value)
}

#[cfg(test)]
#[path = "dll_exports_tests.rs"]
mod tests;
