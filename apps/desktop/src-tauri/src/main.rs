#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

mod commands;
mod effects;

use std::sync::Arc;
use tauri::{Emitter, Manager};
use tokio::sync::broadcast::error::RecvError;
use xode_engine::Engine;

/// Toasts from an unpackaged exe are dropped unless its AppUserModelID is registered.
#[cfg(windows)]
fn register_aumid(id: &str) {
    use std::os::windows::process::CommandExt;
    let key = format!(r"HKCU\Software\Classes\AppUserModelId\{id}");
    let _ = std::process::Command::new("reg")
        .args(["add", &key, "/v", "DisplayName", "/t", "REG_SZ", "/d", "Xode", "/f"])
        .creation_flags(0x0800_0000)
        .status();
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new("reg")
            .args(["add", &key, "/v", "IconUri", "/t", "REG_SZ", "/d", &exe.to_string_lossy(), "/f"])
            .creation_flags(0x0800_0000)
            .status();
    }
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            #[cfg(windows)]
            register_aumid(&app.config().identifier);
            let engine: Arc<Engine> = Engine::new().map_err(|e| e.to_string())?;
            let blur = engine.config().theme.blur;
            app.manage(engine.clone());

            let kind = app.get_webview_window("main").map(|w| effects::apply(&w, blur)).unwrap_or("none");
            app.manage(effects::EffectState::new(kind));

            let handle = app.handle().clone();
            let mut rx = engine.subscribe();
            tauri::async_runtime::spawn(async move {
                loop {
                    match rx.recv().await {
                        Ok(ev) => {
                            let _ = handle.emit("xode://event", &ev);
                        }
                        Err(RecvError::Lagged(n)) => {
                            let _ = handle.emit("xode://lagged", n);
                        }
                        Err(RecvError::Closed) => break,
                    }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::config,
            commands::set_config,
            commands::projects,
            commands::add_project,
            commands::update_project,
            commands::remove_project,
            commands::sessions,
            commands::new_session,
            commands::session,
            commands::rename_session,
            commands::delete_session,
            commands::messages,
            commands::send,
            commands::cancel,
            commands::steer,
            commands::unqueue,
            commands::rewind,
            commands::queued,
            commands::is_running,
            commands::command,
            commands::commands,
            commands::set_mode,
            commands::set_model,
            commands::context_view,
            commands::stats,
            commands::permission_reply,
            commands::detect_gateway,
            commands::test_gateway,
            commands::scan_gateways,
            commands::refresh_models,
            commands::list_dir,
            commands::complete_path,
            commands::browser_test,
            commands::mcp_test,
            commands::mcp_status,
            commands::index_stats,
            commands::write_text_file,
            effects::window_effect,
            effects::set_window_effect,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Xode");
}
