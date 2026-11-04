//! Console-based Host service runner managing continuous discovery and graceful teardown.
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use sumibi_host::host_control::{ControlError, Phase};
use sumibi_host::host_service::HostService;
use windows::core::BOOL;
use windows::Win32::System::Console::SetConsoleCtrlHandler;

static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn console_ctrl_handler(_ctrl_type: u32) -> BOOL {
    SHUTDOWN_REQUESTED.store(true, Ordering::Release);
    BOOL(1)
}

pub fn run(path: PathBuf, auto_grant: bool) -> Result<(), &'static str> {
    let mut service = HostService::start(path).map_err(|e| match e {
        ControlError::AlreadyRunning => "Host is already running in this logon session",
        _ => "failed to start host service",
    })?;

    // Wait until initial startup phase settles
    let startup_deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(Some(status)) = service.try_status() {
            if status.phase == Phase::Failed {
                return Err(match status.error {
                    Some(ControlError::AlreadyRunning) => {
                        "Host is already running in this logon session"
                    }
                    Some(ControlError::Startup) => "settings invalid or unavailable",
                    _ => "host service failed during startup",
                });
            }
            if status.phase == Phase::Ready {
                break;
            }
        }
        if std::time::Instant::now() >= startup_deadline {
            return Err("timed out waiting for host startup");
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    if auto_grant {
        if let Ok(Some(status)) = service.try_status() {
            if let Some(offer) = status.offer {
                if offer.can_grant() {
                    let _ = service.control().try_grant(offer);
                    println!("Manual conversion consent: GRANTED (Sending enabled)");
                } else {
                    println!("Manual conversion consent: Unable to grant (check API key)");
                }
            }
        }
    } else {
        println!("Note: Manual conversion sending is currently disabled (privacy protection).");
        println!(
            "      Use '--grant' flag to enable sending: cargo run -p sumibi-host -- run --grant"
        );
    }

    println!("Sumibi Host service running. Press Ctrl+C to stop.");

    // Install Ctrl+C handler
    unsafe {
        let _ = SetConsoleCtrlHandler(Some(console_ctrl_handler), true);
    }

    while !SHUTDOWN_REQUESTED.load(Ordering::Acquire) {
        if let Some(result) = service.try_finish() {
            return match result {
                Ok(()) => Ok(()),
                Err(_) => Err("host worker terminated unexpectedly"),
            };
        }
        if let Ok(Some(status)) = service.try_status() {
            if status.phase == Phase::Failed {
                return Err("host service failed");
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    println!("Shutting down Sumibi Host service...");
    service.stop().map_err(|_| "error during host shutdown")?;
    println!("Sumibi Host service stopped.");
    Ok(())
}
