#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = bypath_desktop_lib::maybe_run_updater_mode(&args) {
        std::process::exit(code);
    }
    if let Some(code) = bypath_desktop_lib::maybe_run_service_mode(&args) {
        std::process::exit(code);
    }
    bypath_desktop_lib::run_with_args(&args);
}
