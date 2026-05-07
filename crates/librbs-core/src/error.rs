use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("I/O error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("parse error in {path}: {message}")]
    Parse { path: PathBuf, message: String },

    #[error("duplicated declaration: {name}")]
    DuplicatedDeclaration { name: String },

    #[error("unknown library: {name}")]
    UnknownLibrary { name: String },

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;
