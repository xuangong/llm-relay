//! Shared core for LLM Relay (UI-agnostic).
pub mod error;
pub use error::AppError;
pub mod config_writer;
pub mod keystore;
pub mod database;
pub use database::Database;
pub mod gateway;
pub mod events;
pub use events::{emit_typed, EventSink, NullSink, SharedEventSink, TRAY_REFRESH_EVENT};
pub mod health;
pub mod proxy_server;
pub mod ipc;
