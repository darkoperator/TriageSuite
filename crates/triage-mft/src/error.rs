use std::path::PathBuf;

use thiserror::Error;

use crate::detect::ArtifactType;

#[derive(Debug, Error)]
pub enum MftriageError {
    #[error("missing file: {0}")]
    MissingFile(PathBuf),
    #[error("unknown artifact type: {0}")]
    UnknownArtifact(PathBuf),
    #[error("unsupported artifact type: {0:?}")]
    UnsupportedArtifact(ArtifactType),
    #[error("malformed artifact {path}: {message}")]
    MalformedArtifact { path: PathBuf, message: String },
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, MftriageError>;
