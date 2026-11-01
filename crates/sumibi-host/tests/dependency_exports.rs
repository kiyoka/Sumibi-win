#![cfg(windows)]
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom};
use std::os::windows::ffi::OsStringExt;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::PathBuf;
use sumibi_ipc::dll_exports::{inspect_file, ExportTarget};
use sumibi_ipc::export_resolver::ExportCatalog;
use sumibi_ipc::host_image::inspect_bytes;
use sumibi_ipc::system_module::{SystemModuleCatalog, SystemModuleLease};
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
};
use windows::Win32::System::SystemInformation::GetSystemDirectoryW;

fn system_fixture(name: &str) -> File {
    // Fixed diagnostic files only; never form a path from a forwarder string.
    assert!(matches!(
        name,
        "advapi32.dll"
            | "bcrypt.dll"
            | "combase.dll"
            | "gdi32.dll"
            | "kernel32.dll"
            | "kernelbase.dll"
            | "ntdll.dll"
            | "oleaut32.dll"
            | "shell32.dll"
            | "user32.dll"
            | "vcruntime140.dll"
    ));
    let mut directory = vec![0u16; 32768];
    // SAFETY: writable bounded buffer, used only to locate a read-only fixture.
    let count = unsafe { GetSystemDirectoryW(Some(&mut directory)) } as usize;
    assert!(count > 0 && count < directory.len());
    let path = PathBuf::from(std::ffi::OsString::from_wide(&directory[..count])).join(name);
    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
        .unwrap();
    let metadata = file.metadata().unwrap();
    assert!(metadata.is_file());
    assert_eq!(
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0,
        0
    );
    file
}

#[test]
fn built_host_kernel32_symbols_match_held_dll_exports_without_loading_it() {
    let host = inspect_bytes(&std::fs::read(env!("CARGO_BIN_EXE_sumibi-host")).unwrap()).unwrap();
    let library = host
        .import_libraries()
        .iter()
        .find(|library| library.name() == "kernel32.dll")
        .unwrap();
    let mut file = system_fixture("kernel32.dll");
    file.seek(SeekFrom::End(0)).unwrap();
    let exports =
        inspect_file(&mut file).expect("system x64 kernel32 must have supported export metadata");
    assert_eq!(exports, inspect_file(&mut file).unwrap());
    let mut direct = 0;
    let mut forwarded = 0;
    for symbol in library.symbols() {
        match exports
            .lookup(symbol)
            .unwrap_or_else(|| panic!("missing kernel32 symbol: {symbol:?}"))
        {
            ExportTarget::Rva(_) => direct += 1,
            ExportTarget::Forwarder(_) => forwarded += 1,
        }
    }
    assert_eq!(direct + forwarded, library.symbols().len());
    println!(
        "Kernel32 import metadata: direct={direct}, unresolved-forwarders={forwarded}, missing=0"
    );
    println!(
        "DLL stayed read-only; no LoadLibrary, forwarder resolution or dependency trust decision."
    );
}

#[test]
fn built_host_kernel32_forwarders_resolve_only_through_explicit_held_dlls() {
    let host = inspect_bytes(&std::fs::read(env!("CARGO_BIN_EXE_sumibi-host")).unwrap()).unwrap();
    let library = host
        .import_libraries()
        .iter()
        .find(|library| library.name() == "kernel32.dll")
        .unwrap();
    let names = ["kernel32.dll", "kernelbase.dll", "ntdll.dll"];
    let mut files: Vec<_> = names.iter().map(|name| system_fixture(name)).collect();
    let metadata: Vec<_> = files
        .iter_mut()
        .map(|file| inspect_file(file).unwrap())
        .collect();
    let mut catalog = ExportCatalog::new();
    for (name, exports) in names.iter().zip(&metadata) {
        catalog.insert(name, exports).unwrap();
    }
    let mut followed = 0;
    let mut max_depth = 0;
    for symbol in library.symbols() {
        let result = catalog
            .resolve("kernel32.dll", symbol)
            .unwrap_or_else(|error| panic!("kernel32 metadata resolution: {symbol:?}: {error:?}"));
        assert_ne!(result.rva(), 0);
        assert!(names.contains(&result.steps().last().unwrap().module()));
        followed += usize::from(result.forwarder_count() != 0);
        max_depth = max_depth.max(result.forwarder_count());
    }
    assert!(files.iter().all(|file| file.metadata().is_ok()));
    println!(
        "Kernel32 metadata graph: resolved={}, forwarded={followed}, max-depth={max_depth}",
        library.symbols().len()
    );
    println!("Only three explicitly held system DLLs used; no loader calls, API-set guesses or signature trust asserted.");
}

#[test]
fn built_host_non_api_set_symbols_inventory() {
    let host = inspect_bytes(&std::fs::read(env!("CARGO_BIN_EXE_sumibi-host")).unwrap()).unwrap();
    let non_api_sets: Vec<_> = host
        .import_libraries()
        .iter()
        .filter(|lib| !lib.name().starts_with("api-") && !lib.name().starts_with("ext-"))
        .collect();

    for lib in &non_api_sets {
        let mut file = system_fixture(lib.name());
        let exports = inspect_file(&mut file)
            .unwrap_or_else(|err| panic!("failed to inspect exports for {}: {err:?}", lib.name()));
        let mut direct = 0;
        let mut forwarded = 0;
        let mut missing = 0;
        let mut forward_targets = Vec::new();
        for sym in lib.symbols() {
            match exports.lookup(sym) {
                Some(ExportTarget::Rva(_)) => direct += 1,
                Some(ExportTarget::Forwarder(target)) => {
                    forwarded += 1;
                    forward_targets.push((sym, target));
                }
                None => missing += 1,
            }
        }
        println!(
            "{}: total={}, direct={}, forwarded={}, missing={}",
            lib.name(),
            lib.symbols().len(),
            direct,
            forwarded,
            missing
        );
        for (sym, target) in &forward_targets {
            println!("  forwarded: {sym:?} -> {target}");
        }
        assert_eq!(missing, 0, "missing symbols in {}", lib.name());
    }
}

#[test]
fn built_host_non_api_set_imports_resolve_completely_through_leased_system_modules() {
    let host = inspect_bytes(&std::fs::read(env!("CARGO_BIN_EXE_sumibi-host")).unwrap()).unwrap();
    let non_api_sets: Vec<_> = host
        .import_libraries()
        .iter()
        .filter(|lib| !lib.name().starts_with("api-") && !lib.name().starts_with("ext-"))
        .collect();

    // Collect all required system module names: imported modules plus known forwarders.
    let mut required_modules = std::collections::BTreeSet::new();
    for lib in &non_api_sets {
        required_modules.insert(lib.name().to_string());
    }
    // KernelBase is needed for forwarder targets from Kernel32.
    required_modules.insert("kernelbase.dll".to_string());

    let mut catalog = SystemModuleCatalog::new();
    for name in &required_modules {
        let lease = SystemModuleLease::open(name)
            .unwrap_or_else(|err| panic!("failed to open system module lease for {name}: {err:?}"));
        catalog.insert(lease).unwrap();
    }
    assert_eq!(catalog.len(), required_modules.len());

    let mut resolved_count = 0;
    let mut forwarder_count = 0;
    let mut max_depth = 0;

    for lib in &non_api_sets {
        for symbol in lib.symbols() {
            let resolved = catalog.resolve(lib.name(), symbol).unwrap_or_else(|err| {
                panic!(
                    "failed to resolve symbol {symbol:?} in module {}: {err:?}",
                    lib.name()
                )
            });
            assert_ne!(resolved.rva(), 0);
            assert!(catalog.contains(resolved.steps().last().unwrap().module()));
            resolved_count += 1;
            if resolved.forwarder_count() > 0 {
                forwarder_count += 1;
                max_depth = max_depth.max(resolved.forwarder_count());
            }
        }
    }

    // Verify all file leases are still active and valid
    for lease in catalog.leases() {
        assert!(lease.file().metadata().is_ok());
    }

    println!(
        "Non-API-set system modules lease catalog: modules={}, resolved={}, forwarded={}, max-depth={max_depth}",
        catalog.len(),
        resolved_count,
        forwarder_count
    );
    println!("All non-API-set static imports resolved through securely leased system DLLs; no LoadLibrary, dynamic resolution, or process execution.");
}
