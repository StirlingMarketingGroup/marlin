use std::fmt;

/// A failure from the core or from a provider. Adapters map these onto an `errno` or an `NTSTATUS`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VfsError {
    NotFound,
    AccessDenied,
    ReadOnly,
    Conflict,
    NotAFolder,
    IsAFolder,
    NoSpace,
    SignInNeeded,
    TimedOut,
    Offline,
    NotSupported,
    StaleHandle,
    /// Any other failure. The text must hold no credential, token, or file contents.
    Other(String),
}

impl fmt::Display for VfsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotFound => "not found",
            Self::AccessDenied => "access denied",
            Self::ReadOnly => "read-only",
            Self::Conflict => "conflict",
            Self::NotAFolder => "not a folder",
            Self::IsAFolder => "is a folder",
            Self::NoSpace => "no space",
            Self::SignInNeeded => "sign-in needed",
            Self::TimedOut => "timed out",
            Self::Offline => "offline",
            Self::NotSupported => "not supported",
            Self::StaleHandle => "stale handle",
            Self::Other(text) => text,
        })
    }
}

impl std::error::Error for VfsError {}
