#![cfg(windows)]

use sumibi_ipc::host_image::{inspect_bytes, HostSubsystem, ImportSymbol};

#[test]
fn built_host_has_supported_native_image_and_static_import_inventory() {
    let bytes = std::fs::read(env!("CARGO_BIN_EXE_sumibi-host")).unwrap();
    let image = inspect_bytes(&bytes).expect("built Host must match the narrow x64 image profile");
    // The logon launch must not allocate a console window.
    assert_eq!(image.subsystem(), HostSubsystem::Gui);
    assert_ne!(image.entry_point_rva(), 0);
    assert!(image.imports().iter().any(|name| name == "kernel32.dll"));
    assert!(image.imports().windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(image.imports().len(), image.import_libraries().len());
    let kernel = image
        .import_libraries()
        .iter()
        .find(|library| library.name() == "kernel32.dll")
        .unwrap();
    assert!(kernel
        .symbols()
        .iter()
        .any(|symbol| matches!(symbol, ImportSymbol::Name(name) if name == "GetCurrentProcessId")));
    let symbol_count: usize = image
        .import_libraries()
        .iter()
        .map(|library| library.symbols().len())
        .sum();
    assert!(symbol_count > 0);
    println!(
        "Built Host static import names: {}",
        image.imports().join(", ")
    );
    println!("Built Host static import symbols: {symbol_count}");
    println!("PE metadata only; no code loaded and no dependency trust asserted.");
}

#[test]
fn built_host_api_set_imports_inventory() {
    let bytes = std::fs::read(env!("CARGO_BIN_EXE_sumibi-host")).unwrap();
    let image = inspect_bytes(&bytes).expect("built Host must match the narrow x64 image profile");
    let api_sets: Vec<_> = image
        .import_libraries()
        .iter()
        .filter(|lib| lib.name().starts_with("api-") || lib.name().starts_with("ext-"))
        .collect();

    let mut total_symbols = 0;
    for lib in &api_sets {
        println!("{}: {} symbols", lib.name(), lib.symbols().len());
        for sym in lib.symbols() {
            println!("  {sym:?}");
            total_symbols += 1;
        }
    }
    println!("Total API set symbols: {total_symbols}");
}
