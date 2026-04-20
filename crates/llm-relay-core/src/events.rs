//! Trait for emitting domain events to whichever frontend is listening.
//! Tauri GUI implements this by calling `AppHandle::emit`. The IPC server
//! implements it by broadcasting `ServerFrame::Event` to all subscribed clients.
//!
//! # Reserved event names
//!
//! Some event names are consumed by the GUI sink itself and not forwarded to
//! the JS frontend or IPC clients. Use the constants below to refer to them
//! so call sites stay searchable.

use serde::Serialize;
use std::sync::Arc;

/// Reserved event: signals the Tauri-side tray menu should be rebuilt.
/// `TauriSink` intercepts this and calls its tray-refresh function instead
/// of forwarding to the JS frontend. Other sinks (IPC, Null) may ignore it.
pub const TRAY_REFRESH_EVENT: &str = "tray-refresh";

/// A name + JSON payload event. Implementations decide where it goes.
pub trait EventSink: Send + Sync + 'static {
    fn emit(&self, name: &str, payload: serde_json::Value);
}

pub type SharedEventSink = Arc<dyn EventSink>;

/// No-op sink for tests / headless contexts where no listener exists yet.
pub struct NullSink;

impl EventSink for NullSink {
    fn emit(&self, _name: &str, _payload: serde_json::Value) {}
}

/// Helper: serialize a typed payload then forward.
pub fn emit_typed<T: Serialize>(sink: &dyn EventSink, name: &str, payload: &T) {
    match serde_json::to_value(payload) {
        Ok(v) => sink.emit(name, v),
        Err(e) => log::error!("emit_typed serialize failed for {name}: {e}"),
    }
}
