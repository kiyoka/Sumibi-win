//! Explicitly launched broker entry. Default builds never enable a converter.
#[cfg(windows)]
fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let result = sumibi_ipc::bootstrap::Bootstrap::parse(&args).and_then(|bootstrap| {
        if bootstrap.is_policy_runtime_diagnostic() {
            sumibi_host::broker_runtime::serve_diagnostic(&bootstrap, cfg!(feature = "spike"))
        } else {
            sumibi_ipc::broker::serve(bootstrap, cfg!(feature = "spike"))
        }
    });
    if let Err(error) = result {
        // Fixed classifications only; no command line, frame, name or SID.
        eprintln!("{error}");
        std::process::exit(1);
    }
}
#[cfg(not(windows))]
fn main() {
    eprintln!("sumibi-host requires Windows");
    std::process::exit(1);
}
