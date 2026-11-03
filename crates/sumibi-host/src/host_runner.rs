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

pub fn run(path: PathBuf) -> Result<(), &'static str> {
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
