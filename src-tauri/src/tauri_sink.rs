use llm_relay_core::{EventSink, TRAY_REFRESH_EVENT};
use tauri::{AppHandle, Emitter, Manager};

pub struct TauriSink {
    handle: AppHandle,
}

impl TauriSink {
    pub fn new(handle: AppHandle) -> Self {
        Self { handle }
    }
}

impl EventSink for TauriSink {
    fn emit(&self, name: &str, payload: serde_json::Value) {
        // Intercept reserved tray-refresh event: GUI-only side effect, not forwarded to JS.
        if name == TRAY_REFRESH_EVENT {
            crate::tray::refresh_tray_menu(&self.handle);
            return;
        }

        // For all other events, try the main window first so window-scoped
        // listeners receive it; fall back to app-wide emit if the webview
        // hasn't been created yet (e.g. during early startup).
        if let Some(window) = self.handle.get_webview_window("main") {
            if let Err(e) = window.emit(name, &payload) {
                log::warn!("tauri window emit {name} failed: {e}");
            }
        } else if let Err(e) = self.handle.emit(name, &payload) {
            log::warn!("tauri emit {name} failed: {e}");
        }
    }
}
