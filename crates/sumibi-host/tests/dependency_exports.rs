#![cfg(windows)]
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom};
use std::os::windows::ffi::OsStringExt;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::PathBuf;
use sumibi_ipc::dll_exports::{inspect_file, ExportTarget};
use sumibi_ipc::export_resolver::ExportCatalog;
use sumibi_ipc::host_image::inspect_bytes;
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
};
use windows::Win32::System::SystemInformation::GetSystemDirectoryW;

fn system_fixture(name: &str) -> File {
    // Fixed diagnostic files only; never form a path from a forwarder string.
    assert!(matches!(
        name,
        "kernel32.dll" | "kernelbase.dll" | "ntdll.dll"
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
