//! Explicitly launched broker entry. Default builds never enable a converter.
#![cfg_attr(windows, windows_subsystem = "windows")]
#[cfg(windows)]
mod consent_window;
#[cfg(windows)]
mod host_paths;
#[cfg(windows)]
mod host_runner;
#[cfg(windows)]
mod settings_init;
#[cfg(windows)]
mod settings_window;
#[cfg(windows)]
mod tray;
#[cfg(windows)]
fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();

    let is_minimized_flag =
        |arg: &std::ffi::OsString| arg == "--minimized" || arg == "--tray" || arg == "-m";
    let start_minimized = args.iter().any(is_minimized_flag);
    let non_flag_args: Vec<_> = args.iter().filter(|arg| !is_minimized_flag(arg)).collect();

    // Shell and logon launches need no console. Explicit CLI invocations can
    // reuse their parent's console without opening a new window. Preserve
    // inherited pipes when stdout or stderr has been redirected.
    if non_flag_args
        .first()
        .is_some_and(|arg| **arg != "host-window" && **arg != "settings-window")
    {
        use windows::Win32::System::Console::{
            AttachConsole, GetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
        };
        unsafe {
            if [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE]
                .into_iter()
                .all(|id| GetStdHandle(id).map_or(true, |handle| handle.is_invalid()))
            {
                let _ = AttachConsole(ATTACH_PARENT_PROCESS);
            }
        }
    }

    if non_flag_args.is_empty() {
        if consent_window::open_existing(!start_minimized) {
            return;
        }
        let path = match host_paths::settings_path() {
            Ok(path) => path,
            Err(()) => {
                eprintln!("settings location unavailable");
                std::process::exit(1);
            }
        };
        // If configured, launch the consent window and background service.
        // Otherwise, launch the settings window to allow initial setup.
        let configured = path.exists();
        let show_consent = if configured {
            true
        } else {
            match settings_window::run_initial_setup(path.clone()) {
                Ok(continue_setup) => continue_setup,
                Err(message) => {
                    eprintln!("{message}");
                    std::process::exit(1);
                }
            }
        };
        if show_consent {
            if let Err(message) = consent_window::run(path, configured && start_minimized) {
                eprintln!("{message}");
                std::process::exit(1);
            }
        }
        return;
    }

    if non_flag_args
        .first()
        .is_some_and(|arg| **arg == "--help" || **arg == "-h" || **arg == "help")
    {
        println!("Sumibi Host service and settings management");
        println!();
        println!("Usage: sumibi-host [command] [options...]");
        println!();
        println!("Commands:");
        println!("  (none)                    Launch GUI (host-window, or settings-window if unconfigured)");
        println!("  run [--grant] [settings-path] Run continuous Host service in console mode (--grant enables manual sending)");
        println!("  host-window [path]        Open GUI consent manager and run background service");
        println!("  settings-window [path]    Open GUI settings and API key manager");
        println!("  settings-init <host> <model> [base-path] Initialize settings file");
        println!("  startup-check [path]      Verify settings and credential status");
        println!("  help, --help, -h          Show this help message");
        println!();
        println!("Options:");
        println!(
            "  --minimized, --tray, -m   Start minimized to system tray without displaying window"
        );
        return;
    }

    if non_flag_args
        .first()
        .is_some_and(|arg| **arg == "run" || **arg == "host-run")
    {
        let auto_grant = args.iter().any(|arg| arg == "--grant" || arg == "-g");
        let remaining: Vec<_> = non_flag_args
            .iter()
            .skip(1)
            .filter(|arg| ***arg != "--grant" && ***arg != "-g")
            .collect();
        let path = match remaining.len() {
            0 => host_paths::settings_path().ok(),
            1 => Some(std::path::PathBuf::from(remaining[0])),
            _ => None,
        };
        if let Err(message) = path
            .ok_or("usage: sumibi-host run [--grant] [absolute-settings-path]")
            .and_then(|p| host_runner::run(p, auto_grant))
        {
            eprintln!("{message}");
            std::process::exit(1);
        }
        return;
    }
    if non_flag_args
        .first()
        .is_some_and(|arg| **arg == "host-window")
    {
        let path = match non_flag_args.len() {
            1 => host_paths::settings_path().ok(),
            2 => Some(std::path::PathBuf::from(non_flag_args[1])),
            _ => None,
        };
        if let Err(message) = path
            .ok_or("usage: sumibi-host host-window [--minimized] [absolute-settings-path]")
            .and_then(|p| consent_window::run(p, start_minimized))
        {
            eprintln!("{message}");
            std::process::exit(1);
        }
        return;
    }
    if non_flag_args
        .first()
        .is_some_and(|arg| **arg == "settings-window")
    {
        let path = match non_flag_args.len() {
            1 => host_paths::settings_path().ok(),
            2 => Some(std::path::PathBuf::from(non_flag_args[1])),
            _ => None,
        };
        let result = path
            .ok_or("settings location unavailable")
            .and_then(settings_window::run);
        if let Err(message) = result {
            eprintln!("{message}");
            std::process::exit(1);
        }
        return;
    }
    if args.first().is_some_and(|arg| arg == "settings-init") {
        if let Err(message) = settings_init::run(&args[1..]) {
            eprintln!("{message}");
            std::process::exit(1);
        }
        println!("settings created; sending requires explicit consent");
        return;
    }
    if args.first().is_some_and(|arg| arg == "startup-check") {
        if args.len() > 2 {
            eprintln!("usage: sumibi-host startup-check [absolute-settings-path]");
            std::process::exit(1);
        }
        let path = if let Some(path) = args.get(1) {
            std::path::PathBuf::from(path)
        } else {
            match host_paths::settings_path() {
                Ok(path) => path,
                Err(()) => {
                    eprintln!("startup settings location unavailable");
                    std::process::exit(1);
                }
            }
        };
        // A standalone startup check has no UI or TSF callbacks. It reads
        // settings and credentials, creates no listener and sends no request.
        let host = match sumibi_host::startup::PreparedHost::load(&path) {
            Ok(host) => host,
            Err(_) => {
                eprintln!("startup settings unavailable or invalid");
                std::process::exit(1);
            }
        };
        use sumibi_host::manual::PolicySource;
        let state = host.authority().snapshot().expect("new startup authority");
        if host.credential_outcome().is_err() {
            eprintln!("settings loaded; credential unavailable; sending disabled");
            std::process::exit(2);
        }
        if state.credential.is_some() {
            println!("settings loaded; credential present; sending disabled");
        } else {
            println!("settings loaded; credential missing; sending disabled");
        }
        return;
    }
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
