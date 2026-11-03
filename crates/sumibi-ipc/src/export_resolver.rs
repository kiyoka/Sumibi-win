//! Bounded metadata-only forwarder graph over explicitly supplied DLL exports.
//! No file search, loader calls, API-set guesses, signature or launch authority.
use crate::dll_exports::{DllExports, ExportTarget};
use crate::host_image::ImportSymbol;
use std::collections::{BTreeMap, BTreeSet};

const MAX_MODULES: usize = 128;
const MAX_FORWARDERS: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolveError {
    InvalidModule,
    InvalidSymbol,
    InvalidForwarder,
    DuplicateModule,
    Capacity,
    ModuleMissing,
    SymbolMissing,
    ApiSetUnresolved,
    Cycle,
    DepthExceeded,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportStep {
    module: String,
    symbol: ImportSymbol,
}
impl ExportStep {
    pub fn module(&self) -> &str {
        &self.module
    }
    pub fn symbol(&self) -> &ImportSymbol {
        &self.symbol
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedExport {
    rva: u32,
    steps: Vec<ExportStep>,
}
impl ResolvedExport {
    /// Image-relative metadata, not a pointer, file identity or trust receipt.
    pub fn rva(&self) -> u32 {
        self.rva
    }
    /// Includes the original lookup and the terminal module / symbol.
    pub fn steps(&self) -> &[ExportStep] {
        &self.steps
    }
    pub fn forwarder_count(&self) -> usize {
        self.steps.len() - 1
    }
}

/// Borrowed immutable metadata only. The caller must bind each name to its
/// intended file and retain all required file / signature / placement leases.
/// The DLL's self-reported module name is never used as a catalog identity.
#[derive(Default)]
pub struct ExportCatalog<'a> {
    modules: BTreeMap<String, &'a DllExports>,
}
impl<'a> ExportCatalog<'a> {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn insert(&mut self, module: &str, exports: &'a DllExports) -> Result<(), ResolveError> {
        let module = normalize_module_name(module)?;
        if self.modules.contains_key(&module) {
            return Err(ResolveError::DuplicateModule);
        }
        if self.modules.len() == MAX_MODULES {
            return Err(ResolveError::Capacity);
        }
        self.modules.insert(module, exports);
        Ok(())
    }
    pub fn resolve(
        &self,
        module: &str,
        symbol: &ImportSymbol,
    ) -> Result<ResolvedExport, ResolveError> {
        let mut module = normalize_module_name(module)?;
        validate_symbol(symbol)?;
        let mut symbol = symbol.clone();
        let mut seen = BTreeSet::new();
        let mut steps = Vec::new();
        for depth in 0..=MAX_FORWARDERS {
            let exports = self
                .modules
                .get(&module)
                .ok_or(ResolveError::ModuleMissing)?;
            let (slot, target) = exports
                .lookup_slot(&symbol)
                .ok_or(ResolveError::SymbolMissing)?;
            // Slot identity detects cycles through aliases and name/ordinal
            // switches, as well as ordinary case-insensitive module cycles.
            if !seen.insert((module.clone(), slot)) {
                return Err(ResolveError::Cycle);
            }
            steps.push(ExportStep {
                module: module.clone(),
                symbol: symbol.clone(),
            });
            match target {
                ExportTarget::Rva(rva) => return Ok(ResolvedExport { rva: *rva, steps }),
                ExportTarget::Forwarder(value) => {
                    if depth == MAX_FORWARDERS {
                        return Err(ResolveError::DepthExceeded);
                    }
                    (module, symbol) = forwarder(value)?;
                }
            }
        }
        Err(ResolveError::DepthExceeded)
    }
}

pub fn normalize_module_name(value: &str) -> Result<String, ResolveError> {
    if value.is_empty() || value.len() > 128 || !value.is_ascii() {
        return Err(ResolveError::InvalidModule);
    }
    let value = value.to_ascii_lowercase();
    let base = value.strip_suffix(".dll").unwrap_or(&value);
    let api_set = base.starts_with("api-") || base.starts_with("ext-");
    if base.is_empty()
        || base.len() + 4 > 128
        || !base.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_') || (api_set && byte == b'~')
        })
    {
        return Err(ResolveError::InvalidModule);
    }
    if api_set {
        return Err(ResolveError::ApiSetUnresolved);
    }
    Ok(format!("{base}.dll"))
}

fn validate_symbol(symbol: &ImportSymbol) -> Result<(), ResolveError> {
    if let ImportSymbol::Name(value) = symbol {
        if value.is_empty()
            || value.len() > 512
            || !value.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err(ResolveError::InvalidSymbol);
        }
    }
    Ok(())
}

fn forwarder(value: &str) -> Result<(String, ImportSymbol), ResolveError> {
    if value.is_empty() || value.len() > 512 || !value.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(ResolveError::InvalidForwarder);
    }
    let (module, target) = value
        .split_once('.')
        .ok_or(ResolveError::InvalidForwarder)?;
    // This narrow profile accepts MODULE.Symbol / MODULE.#ordinal only.
    // Reject ambiguous extra dots (including explicit .dll) rather than guess.
    if target.contains('.') {
        return Err(ResolveError::InvalidForwarder);
    }
    let module = normalize_module_name(module).map_err(|error| match error {
        ResolveError::ApiSetUnresolved => error,
        _ => ResolveError::InvalidForwarder,
    })?;
    let symbol = if let Some(digits) = target.strip_prefix('#') {
        if digits.is_empty()
            || digits.len() > 5
            || !digits.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(ResolveError::InvalidForwarder);
        }
        ImportSymbol::Ordinal(digits.parse().map_err(|_| ResolveError::InvalidForwarder)?)
    } else {
        ImportSymbol::Name(target.to_owned())
    };
    validate_symbol(&symbol).map_err(|_| ResolveError::InvalidForwarder)?;
    Ok((module, symbol))
}
