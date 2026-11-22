#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

fn main() {
    // Velopack MUST be the very first statement in main() so install/update/uninstall
    // lifecycle hooks execute cleanly before WebView2 / Tauri initializes.
    velopack::VelopackApp::build().run();

    ctrlmate_desktop_lib::run();
}
