use std::ffi::OsString;
use std::path::PathBuf;
use sumibi_host::manual::Scheme;
use sumibi_host::settings::{Settings, SettingsError};

pub fn run(args: &[OsString]) -> Result<(), &'static str> {
    let (explicit_path, values) = if args.first().is_some_and(|arg| arg == "--settings") {
        if args.len() < 2 {
            return Err(USAGE);
        }
        (Some(PathBuf::from(&args[1])), &args[2..])
    } else {
        (None, args)
    };
    if !(2..=3).contains(&values.len()) {
        return Err(USAGE);
    }
    let host = values[0].to_str().ok_or("invalid settings")?;
    let model = values[1].to_str().ok_or("invalid settings")?;
    let base = values
        .get(2)
        .map(|value| value.to_str().ok_or("invalid settings"))
        .transpose()?
        .unwrap_or("/");
    let settings =
        Settings::new(Scheme::Https, host, 443, base, model).map_err(|_| "invalid settings")?;
    let path = match explicit_path {
        Some(path) => path,
        None => {
            let path =
                crate::host_paths::settings_path().map_err(|_| "settings location unavailable")?;
            let parent = path.parent().ok_or("settings location unavailable")?;
            // Only the one application directory is created; LocalAppData
            // must already exist. No recursive creation or fallback location.
            match std::fs::create_dir(parent) {
                Ok(()) => {}
                Err(error)
                    if error.kind() == std::io::ErrorKind::AlreadyExists && parent.is_dir() => {}
                Err(_) => return Err("settings directory unavailable"),
            }
            path
        }
    };
    match settings.create(&path) {
        Ok(()) => Ok(()),
        Err(SettingsError::AlreadyExists) => Err("settings already exist; no changes made"),
        Err(_) => Err("settings creation failed"),
    }
}

const USAGE: &str =
    "usage: sumibi-host settings-init [--settings absolute-path] <https-host> <model> [base-path]";
