#[cfg(windows)]
mod broker_bridge;
#[cfg(windows)]
mod broker_http_fixture;
#[cfg(windows)]
mod broker_process_fixture;
#[cfg(windows)]
mod broker_shared_fixture;
#[cfg(windows)]
mod channel_probe;
#[cfg(windows)]
mod edit_host;
#[cfg(windows)]
mod fanout_probe;
#[cfg(windows)]
mod input_signal;
#[cfg(windows)]
mod live_render;
#[cfg(windows)]
mod pipe_peer;
#[cfg(windows)]
mod pipe_probe;
#[cfg(windows)]
mod probe;
#[cfg(windows)]
mod recovery_view;
#[cfg(windows)]
mod registration;
#[cfg(windows)]
mod registration_backend;
#[cfg(windows)]
mod registration_fixture;
#[cfg(windows)]
mod registration_native;
#[cfg(windows)]
mod registration_registry;
#[cfg(windows)]
mod render;
#[cfg(windows)]
mod system_registration;
#[cfg(windows)]
mod tsf_registry_fixture;
#[cfg(windows)]
mod window_host;
#[cfg(windows)]
mod worker_probe;

#[cfg(windows)]
fn main() {
    if let Err(error) = probe::run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("sumibi-setup requires Windows x64");
    std::process::exit(1);
}
