use std::fmt;
use std::path::PathBuf;

#[derive(Debug)]
pub enum ArchiveError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The volume is below the configured free-space floor; nothing was written.
    InsufficientSpace {
        path: PathBuf,
        free: u64,
        floor: u64,
    },
    /// Not an archive file, an unsupported version, or a corrupt record.
    Format {
        path: PathBuf,
        message: String,
    },
    InvalidSymbol(String),
    Manifest {
        path: PathBuf,
        message: String,
    },
    NotFound(PathBuf),
}

impl ArchiveError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        ArchiveError::Io {
            path: path.into(),
            source,
        }
    }

    pub(crate) fn format(path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
        ArchiveError::Format {
            path: path.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArchiveError::Io { path, source } => write!(f, "{}: {source}", path.display()),
            ArchiveError::InsufficientSpace { path, free, floor } => write!(
                f,
                "{}: refusing to write: {} MiB free is below the floor of {} MiB (ARCHIVE_MIN_FREE_MB)",
                path.display(),
                free / (1024 * 1024),
                floor / (1024 * 1024)
            ),
            ArchiveError::Format { path, message } => {
                write!(f, "{}: invalid archive file: {message}", path.display())
            }
            ArchiveError::InvalidSymbol(s) => write!(f, "invalid symbol {s:?}"),
            ArchiveError::Manifest { path, message } => {
                write!(f, "{}: invalid manifest: {message}", path.display())
            }
            ArchiveError::NotFound(p) => write!(f, "{}: not found", p.display()),
        }
    }
}

impl std::error::Error for ArchiveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ArchiveError::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
