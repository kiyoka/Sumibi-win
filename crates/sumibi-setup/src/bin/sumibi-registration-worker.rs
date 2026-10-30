//! Dedicated native registration test worker: no static MSCTF imports, no UI.
#[cfg(windows)]
#[path = "../registration_backend.rs"]
mod registration_backend;
#[cfg(windows)]
#[path = "../registration_native.rs"]
mod registration_native;
#[cfg(windows)]
#[path = "../registration_registry.rs"]
mod registration_registry;
#[cfg(windows)]
#[path = "../tsf_registry_fixture.rs"]
mod tsf_registry_fixture;

fn main() {
    #[cfg(windows)]
    {
        let args: Vec<_> = std::env::args_os().skip(1).collect();
        let result = if args
            .first()
            .is_some_and(|arg| arg == "tsf-registry-fixture")
        {
            tsf_registry_fixture::run(&args[1..])
        } else {
            Err("This binary only runs the isolated native registration fixture".into())
        };
        if let Err(error) = result {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
    #[cfg(not(windows))]
    {
        eprintln!("Native registration fixtures require Windows x64");
        std::process::exit(1);
    }
}
