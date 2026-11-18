// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Finder does not inherit the shell's PATH. Set it before Tauri starts threads.
    #[cfg(target_os = "macos")]
    if let Err(error) = fix_path_env::fix() {
        eprintln!("Failed to read shell PATH: {error}");
    }
    ox::run();
}
