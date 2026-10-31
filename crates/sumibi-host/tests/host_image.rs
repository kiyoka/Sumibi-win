#![cfg(windows)]

use sumibi_ipc::host_image::{inspect_bytes, HostSubsystem};

#[test]
fn built_host_has_supported_native_image_and_static_import_inventory() {
    let bytes = std::fs::read(env!("CARGO_BIN_EXE_sumibi-host")).unwrap();
    let image = inspect_bytes(&bytes).expect("built Host must match the narrow x64 image profile");
    assert_eq!(image.subsystem(), HostSubsystem::Console);
    assert_ne!(image.entry_point_rva(), 0);
    assert!(image.imports().iter().any(|name| name == "kernel32.dll"));
    assert!(image.imports().windows(2).all(|pair| pair[0] < pair[1]));
    println!(
        "Built Host static import names: {}",
        image.imports().join(", ")
    );
    println!("PE metadata only; no code loaded and no dependency trust asserted.");
}
