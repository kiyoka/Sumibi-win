//! Two authenticated pipe instances in one disposable Broker process.
use sumibi_ipc::{broker::diagnostic_two_client_probe, Error, Result};

pub(super) fn run(default_host: bool) -> Result<()> {
    // Process launch, pipe I/O and child cleanup stay off the TSF owner STA.
    std::thread::spawn(move || {
        let path = std::env::current_exe()
            .map_err(|_| Error::InvalidArgument)?
            .parent()
            .ok_or(Error::InvalidArgument)?
            .join("sumibi-host.exe");
        let result = diagnostic_two_client_probe(&path);
        if default_host {
            if !matches!(result, Err(Error::Disconnected | Error::PolicyDenied)) {
                return Err(Error::InvalidMessage);
            }
            println!("Broker fanout / default Host rejects two-client diagnostic service: OK");
        } else {
            result?;
            println!("Broker fanout / two pipe orders, revoke, regrant, isolated retirement, failed notice write: OK");
        }
        Ok(())
    })
    .join()
    .map_err(|_| Error::WorkerFailed)?
}
