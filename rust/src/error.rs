//! The one error type the library returns.

use std::fmt;

/// Anything that stops a comparison: a missing file, a key that is not in both
/// headers, an engine that cannot run.
#[derive(Debug)]
pub struct Error(pub String);

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Error(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error(e.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error(e.to_string())
    }
}

impl From<duckdb::Error> for Error {
    fn from(e: duckdb::Error) -> Self {
        Error(e.to_string())
    }
}

impl From<csv::Error> for Error {
    fn from(e: csv::Error) -> Self {
        Error(e.to_string())
    }
}

impl From<polars::prelude::PolarsError> for Error {
    fn from(e: polars::prelude::PolarsError) -> Self {
        Error(e.to_string())
    }
}

/// The library's result type.
pub type Result<T> = std::result::Result<T, Error>;
