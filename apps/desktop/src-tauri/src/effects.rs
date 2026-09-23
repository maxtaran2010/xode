//! Native window backdrop (Mica / Acrylic / Blur on Windows, vibrancy on macOS).
use std::sync::Mutex;
use tauri::{State, WebviewWindow};

pub struct EffectState(Mutex<&'static str>);

impl EffectState {
    pub fn new(kind: &'static str) -> Self {
        Self(Mutex::new(kind))
    }
}

/// Applies the best supported backdrop. Returns "mica" | "acrylic" | "blur" | "vibrancy" | "none".
pub fn apply(win: &WebviewWindow, enabled: bool) -> &'static str {
    clear(win);
    if !enabled {
        return "none";
    }
    #[cfg(target_os = "windows")]
    {
        use window_vibrancy::{apply_blur, apply_mica};
        // Mica on Windows 11. Acrylic is skipped on purpose: on Windows 10 it makes
        // dragging and resizing the window extremely slow; plain blur has no such cost.
        if apply_mica(win, Some(true)).is_ok() {
            return "mica";
        }
        if apply_blur(win, Some((18, 22, 30, 170))).is_ok() {
            return "blur";
        }
        "none"
    }
    #[cfg(target_os = "macos")]
    {
        use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial, NSVisualEffectState};
        match apply_vibrancy(win, NSVisualEffectMaterial::UnderWindowBackground, Some(NSVisualEffectState::Active), None) {
            Ok(_) => "vibrancy",
            Err(_) => "none",
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = win;
        "none"
    }
}

fn clear(win: &WebviewWindow) {
    #[cfg(target_os = "windows")]
    {
        let _ = window_vibrancy::clear_mica(win);
        let _ = window_vibrancy::clear_acrylic(win);
        let _ = window_vibrancy::clear_blur(win);
    }
    #[cfg(target_os = "macos")]
    {
        let _ = window_vibrancy::clear_vibrancy(win);
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = win;
    }
}

/// Current backdrop kind (sync command: runs on the main thread).
#[tauri::command]
pub fn window_effect(state: State<EffectState>) -> String {
    state.0.lock().map(|k| k.to_string()).unwrap_or_else(|_| "none".into())
}

/// Enables/disables the backdrop at runtime (sync command: runs on the main thread).
#[tauri::command]
pub fn set_window_effect(window: WebviewWindow, state: State<EffectState>, enabled: bool) -> String {
    let kind = apply(&window, enabled);
    if let Ok(mut k) = state.0.lock() {
        *k = kind;
    }
    kind.to_string()
}
