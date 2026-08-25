use thiserror::Error;

#[derive(Debug, Error)]
pub enum EvtxTriageError {
    #[error("I/O error for {path}: {source}")]
    Io {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error("EVTX parse error in {path}: {message}")]
    EvtxParse {
        path: std::path::PathBuf,
        message: String,
    },
    #[error("Map YAML parse error in {path}: {source}")]
    MapYaml {
        path: std::path::PathBuf,
        source: serde_yml::Error,
    },
    #[error("{0}")]
    Argument(String),
}

impl EvtxTriageError {
    pub fn io(path: impl Into<std::path::PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    pub fn evtx_parse(path: impl Into<std::path::PathBuf>, message: impl Into<String>) -> Self {
        Self::EvtxParse {
            path: path.into(),
            message: message.into(),
        }
    }

    pub fn map_yaml(path: impl Into<std::path::PathBuf>, source: serde_yml::Error) -> Self {
        Self::MapYaml {
            path: path.into(),
            source,
        }
    }
}

pub type Result<T> = std::result::Result<T, EvtxTriageError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argument_error_displays_message() {
        let e = EvtxTriageError::Argument("need --csv or --json".into());
        assert_eq!(e.to_string(), "need --csv or --json");
    }

    #[test]
    fn io_error_includes_path() {
        let e = EvtxTriageError::io(
            "/tmp/foo.evtx",
            std::io::Error::new(std::io::ErrorKind::NotFound, "not found"),
        );
        assert!(e.to_string().contains("/tmp/foo.evtx"));
    }
}
