//! Shared core for LLM Relay (UI-agnostic).
pub mod error;
pub use error::AppError;
pub mod config_writer;
pub mod keystore;
pub mod database;
pub use database::Database;
