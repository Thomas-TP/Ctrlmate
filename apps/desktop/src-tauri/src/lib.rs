use ctrlmate_protocol::DEFAULT_IPC_PORT;
use tauri::Manager;

#[tauri::command]
fn get_ipc_endpoint() -> String {
    format!("http://127.0.0.1:{DEFAULT_IPC_PORT}")
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![get_ipc_endpoint])
        .run(tauri::generate_context!())
        .expect("error while running Ctrlmate desktop application");
}
