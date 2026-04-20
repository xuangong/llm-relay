use std::fmt;

#[derive(Debug)]
pub enum AppError {
    Database(String),
    Http(String),
    Io(String),
    Json(String),
    Config(String),
    NotImplemented(String),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::Database(msg) => write!(f, "Database error: {msg}"),
            AppError::Http(msg) => write!(f, "HTTP error: {msg}"),
            AppError::Io(msg) => write!(f, "IO error: {msg}"),
            AppError::Json(msg) => write!(f, "JSON error: {msg}"),
            AppError::Config(msg) => write!(f, "Config error: {msg}"),
            AppError::NotImplemented(msg) => write!(f, "Not yet implemented: {msg}"),
        }
    }
}

impl std::error::Error for AppError {}

impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        AppError::Database(e.to_string())
    }
}

impl From<reqwest::Error> for AppError {
    fn from(e: reqwest::Error) -> Self {
        AppError::Http(e.to_string())
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::Io(e.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::Json(e.to_string())
    }
}

impl From<AppError> for String {
    fn from(e: AppError) -> String {
        e.to_string()
    }
}
