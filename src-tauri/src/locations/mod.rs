use async_trait::async_trait;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, RwLock};

use crate::fs_utils::FileItem;

pub mod archive;
mod file;
pub mod gdrive;
pub mod sftp;
#[cfg(not(target_os = "windows"))]
pub mod smb;

pub use archive::ArchiveProvider;
pub use file::FileSystemProvider;
pub use gdrive::GoogleDriveProvider;
pub use sftp::SftpProvider;
#[cfg(not(target_os = "windows"))]
pub use smb::SmbProvider;

pub type ProviderRef = Arc<dyn LocationProvider + Send + Sync>;

type ProviderMap = HashMap<String, ProviderRef>;

static REGISTRY: Lazy<RwLock<ProviderMap>> = Lazy::new(|| {
    let mut map = HashMap::new();
    let file_provider: ProviderRef = Arc::new(FileSystemProvider::default());
    map.insert(file_provider.scheme().to_string(), file_provider);
    let archive_provider: ProviderRef = Arc::new(ArchiveProvider::default());
    map.insert(archive_provider.scheme().to_string(), archive_provider);
    let gdrive_provider: ProviderRef = Arc::new(GoogleDriveProvider::default());
    map.insert(gdrive_provider.scheme().to_string(), gdrive_provider);
    let sftp_provider: ProviderRef = Arc::new(SftpProvider::default());
    map.insert(sftp_provider.scheme().to_string(), sftp_provider);
    #[cfg(not(target_os = "windows"))]
    {
        let smb_provider: ProviderRef = Arc::new(SmbProvider::default());
        map.insert(smb_provider.scheme().to_string(), smb_provider);
    }
    RwLock::new(map)
});

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocationSummary {
    pub raw: String,
    pub scheme: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authority: Option<String>,
    pub path: String,
    pub display_path: String,
}

impl LocationSummary {
    pub fn new(
        scheme: impl Into<String>,
        authority: Option<String>,
        path: impl Into<String>,
        display_path: impl Into<String>,
    ) -> Self {
        let scheme = scheme.into();
        let path = path.into();
        let display_path = display_path.into();
        let raw = compose_raw_uri(&scheme, authority.as_deref(), &path);
        Self {
            raw,
            scheme,
            authority,
            path,
            display_path,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocationCapabilities {
    pub scheme: String,
    pub display_name: String,
    pub can_read: bool,
    pub can_write: bool,
    pub can_create_directories: bool,
    pub can_delete: bool,
    pub can_rename: bool,
    pub can_copy: bool,
    pub can_move: bool,
    pub supports_watching: bool,
    pub requires_explicit_refresh: bool,
}

impl LocationCapabilities {
    pub fn new(
        scheme: impl Into<String>,
        display_name: impl Into<String>,
        can_read: bool,
        can_write: bool,
    ) -> Self {
        Self {
            scheme: scheme.into(),
            display_name: display_name.into(),
            can_read,
            can_write,
            can_create_directories: can_write,
            can_delete: can_write,
            can_rename: can_write,
            can_copy: can_write,
            can_move: can_write,
            supports_watching: false,
            requires_explicit_refresh: false,
        }
    }

    pub fn with_supports_watching(mut self, supports: bool) -> Self {
        self.supports_watching = supports;
        self
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocationDescriptorInput {
    #[serde(default)]
    pub scheme: Option<String>,
    #[serde(default)]
    pub authority: Option<String>,
    pub path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum LocationInput {
    Raw(String),
    Descriptor(LocationDescriptorInput),
}

#[derive(Debug, Clone)]
pub struct Location {
    scheme: String,
    authority: Option<String>,
    path: String,
    raw: String,
}

impl Location {
    /// Parse a raw URI string into a Location
    #[allow(dead_code)] // Useful utility for future providers and tests
    pub fn parse(raw: &str) -> Result<Location, String> {
        parse_raw_location(raw.to_string())
    }

    pub fn scheme(&self) -> &str {
        &self.scheme
    }

    pub fn authority(&self) -> Option<&str> {
        self.authority.as_deref()
    }

    #[allow(dead_code)] // Used in tests and by future providers
    pub fn path(&self) -> &str {
        &self.path
    }

    #[allow(dead_code)] // Used in tests and by future providers
    pub fn raw(&self) -> &str {
        &self.raw
    }

    pub fn to_path_string(&self) -> String {
        if let Some(authority) = &self.authority {
            let mut path = String::from("//");
            path.push_str(authority);
            if self.path.starts_with('/') {
                path.push_str(&self.path);
            } else {
                path.push('/');
                path.push_str(&self.path);
            }
            path
        } else if self.path.is_empty() {
            "/".to_string()
        } else {
            self.path.clone()
        }
    }
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.raw)
    }
}

fn compose_raw_uri(scheme: &str, authority: Option<&str>, path: &str) -> String {
    let mut raw = format!("{scheme}://");
    if let Some(host) = authority {
        raw.push_str(host);
    }
    if !path.starts_with('/') {
        raw.push('/');
    }
    raw.push_str(path);
    raw
}

impl LocationInput {
    pub fn into_location(self) -> Result<Location, String> {
        match self {
            LocationInput::Raw(value) => parse_raw_location(value),
            LocationInput::Descriptor(descriptor) => {
                let scheme = descriptor
                    .scheme
                    .unwrap_or_else(|| "file".to_string())
                    .to_ascii_lowercase();
                let authority = descriptor.authority.filter(|s| !s.is_empty());
                let path = sanitize_path(descriptor.path);
                let raw = compose_raw_uri(&scheme, authority.as_deref(), &path);
                Ok(Location {
                    scheme,
                    authority,
                    path,
                    raw,
                })
            }
        }
    }
}

fn parse_raw_location(value: String) -> Result<Location, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(Location {
            scheme: "file".to_string(),
            authority: None,
            path: "/".to_string(),
            raw: "file:///".to_string(),
        });
    }

    if let Some(idx) = trimmed.find("://") {
        let scheme = trimmed[..idx].to_ascii_lowercase();
        let remainder = &trimmed[idx + 3..];

        let (authority, path_part) = if remainder.starts_with('/') {
            (None, remainder)
        } else if let Some(slash_idx) = remainder.find('/') {
            let (auth, path) = remainder.split_at(slash_idx);
            (Some(auth.to_string()), path)
        } else {
            (Some(remainder.to_string()), "/")
        };

        let path = sanitize_path(path_part);
        let raw = compose_raw_uri(&scheme, authority.as_deref(), &path);
        Ok(Location {
            scheme,
            authority,
            path,
            raw,
        })
    } else {
        let path = sanitize_path(trimmed);
        let raw = compose_raw_uri("file", None, &path);
        Ok(Location {
            scheme: "file".to_string(),
            authority: None,
            path,
            raw,
        })
    }
}

fn sanitize_path(input: impl Into<String>) -> String {
    let mut value = input.into();
    if value.is_empty() {
        return "/".to_string();
    }
    if value.starts_with("//") {
        // preserve leading double-slash for network paths
        let segments: Vec<&str> = value.splitn(3, '/').collect();
        if segments.len() >= 3 {
            // segments[0] is empty, segments[1] is empty, segments[2] begins host
            // we return //host/rest
            if !segments[2].starts_with('/') {
                return format!("//{}", segments[2]);
            }
        }
    }
    if !value.starts_with('/') {
        value = format!("/{value}");
    }
    if value.len() > 1 && value.ends_with('/') {
        value.pop();
    }
    value
}

pub struct ProviderDirectoryEntries {
    pub location: LocationSummary,
    pub entries: Vec<FileItem>,
}

/// What a folder read answers for a path that is there and is not a folder, such as a file or a link to one. The `file`, `sftp`, and `smb` providers answer it. A navigation that gets the `[ENOTDIR]` code opens the folder that holds the item and selects it.
pub fn not_a_folder(path: &str) -> String {
    crate::commands::format_error(
        crate::commands::error_codes::ENOTDIR,
        &format!("Not a folder: {path}"),
    )
}

#[async_trait]
pub trait LocationProvider: Send + Sync {
    fn scheme(&self) -> &'static str;
    fn capabilities(&self, location: &Location) -> LocationCapabilities;

    async fn read_directory(&self, location: &Location)
        -> Result<ProviderDirectoryEntries, String>;
    async fn get_file_metadata(&self, location: &Location) -> Result<FileItem, String>;
    async fn create_directory(&self, location: &Location) -> Result<(), String>;
    async fn delete(&self, location: &Location) -> Result<(), String>;
    async fn rename(&self, from: &Location, to: &Location) -> Result<(), String>;
    async fn copy(&self, from: &Location, to: &Location) -> Result<(), String>;
    async fn move_item(&self, from: &Location, to: &Location) -> Result<(), String> {
        self.rename(from, to).await
    }
}

pub fn get_provider_for_scheme(scheme: &str) -> Option<ProviderRef> {
    REGISTRY
        .read()
        .expect("Provider registry lock poisoned")
        .get(&scheme.to_ascii_lowercase())
        .cloned()
}

pub fn ensure_provider(scheme: &str) -> Result<ProviderRef, String> {
    get_provider_for_scheme(scheme)
        .ok_or_else(|| format!("No provider registered for scheme '{scheme}'"))
}

pub fn resolve_location(input: LocationInput) -> Result<(ProviderRef, Location), String> {
    let location = input.into_location()?;
    let provider = ensure_provider(location.scheme())?;
    Ok((provider, location))
}

/// The path for a temp copy of the file `name`: `<temp dir>/<area>/<key>/<name>`. Makes the folder.
/// The key keeps two files with one name apart, so the copy keeps the exact name, and a paste, an open, or a drag out shows it.
pub fn temp_copy_path(area: &str, key: &str, name: &str) -> Result<PathBuf, String> {
    // The key is one folder name: a hash, or an ID that came from the frontend.
    let mut parts = Path::new(key).components();
    if !matches!(
        (parts.next(), parts.next()),
        (Some(Component::Normal(_)), None)
    ) {
        return Err(format!("Invalid temp folder name: {key}"));
    }

    // Replace only what one file name cannot hold on this system.
    let forbidden: &[char] = if cfg!(windows) {
        &['/', '\\', ':', '*', '?', '"', '<', '>', '|']
    } else {
        &['/']
    };
    let mut name: String = name
        .chars()
        .map(|c| {
            if c == '\0' || forbidden.contains(&c) || (cfg!(windows) && c.is_control()) {
                '_'
            } else {
                c
            }
        })
        .collect();
    if name.is_empty() || name == "." || name == ".." {
        name = "file".to_string();
    }
    if cfg!(windows) {
        // Windows drops a final dot or space from a name.
        if name.ends_with(['.', ' ']) {
            name.pop();
            name.push('_');
        }
        // Windows opens a device, not a file, for these names, with or without an extension.
        let stem = name
            .split('.')
            .next()
            .unwrap_or("")
            .trim_end()
            .to_uppercase();
        let device = matches!(
            stem.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        ) || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && ["1", "2", "3", "4", "5", "6", "7", "8", "9", "¹", "²", "³"].contains(&&stem[3..]));
        if device {
            name.insert(0, '_');
        }
    }

    // Only this user may open the folder: a shared /tmp holds other users' files too.
    let folder = std::env::temp_dir().join(area).join(key);
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder
        .create(&folder)
        .map_err(|e| format!("Failed to create temp directory: {}", e))?;
    Ok(folder.join(name))
}

/// Delete a temp copy that has a folder of its own, and that folder.
/// Only for a copy no other caller can reach: one under a key made for it alone, such as a random ID.
/// A copy whose download never made the file still has its folder removed.
pub fn remove_temp_copy(path: &Path) -> std::io::Result<()> {
    let removed = match std::fs::remove_file(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    };
    if let Some(folder) = path.parent() {
        let _ = std::fs::remove_dir(folder);
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_a_folder_names_the_path() {
        assert_eq!(not_a_folder("/a/b.txt"), "[ENOTDIR] Not a folder: /a/b.txt");
    }

    #[test]
    fn parse_file_paths() {
        let raw = "/Users/example".to_string();
        let loc = LocationInput::Raw(raw).into_location().unwrap();
        assert_eq!(loc.scheme(), "file");
        assert_eq!(loc.path(), "/Users/example");
        assert_eq!(loc.raw(), "file:///Users/example");
    }

    #[test]
    fn parse_uri_with_authority() {
        let loc = LocationInput::Raw("s3://bucket/path".into())
            .into_location()
            .unwrap();
        assert_eq!(loc.scheme(), "s3");
        assert_eq!(loc.authority(), Some("bucket"));
        assert_eq!(loc.path(), "/path");
        assert_eq!(loc.raw(), "s3://bucket/path");
    }

    #[test]
    fn sanitize_path_edge_cases() {
        // Empty path becomes root
        assert_eq!(sanitize_path(""), "/");

        // Trailing slashes are removed
        assert_eq!(sanitize_path("/foo/"), "/foo");

        // Relative paths get leading slash
        assert_eq!(sanitize_path("foo/bar"), "/foo/bar");

        // Network paths with host are preserved
        assert_eq!(sanitize_path("//host/share"), "//host/share");

        // Double slash without host stays as network path prefix
        assert_eq!(sanitize_path("//"), "//");

        // Triple slash reduces to double (network path prefix)
        assert_eq!(sanitize_path("///"), "//");
    }

    const AREA: &str = "marlin-test-temp-copy";

    fn file_name(path: &Path) -> String {
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap()
            .to_string()
    }

    #[test]
    fn temp_copy_path_keeps_the_file_name() {
        let key = uuid::Uuid::new_v4().to_string();
        let env = temp_copy_path(AREA, &key, ".env").unwrap();
        let notes = temp_copy_path(AREA, &key, "名前 ü.txt").unwrap();
        assert_eq!(file_name(&env), ".env");
        assert_eq!(file_name(&notes), "名前 ü.txt");
        assert!(env.parent().unwrap().is_dir());
        assert_eq!(env.parent(), notes.parent());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(env.parent().unwrap())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700);

            let colon = temp_copy_path(AREA, &key, "notes: draft.txt").unwrap();
            let slash = temp_copy_path(AREA, &key, "back\\slash.txt").unwrap();
            assert_eq!(file_name(&colon), "notes: draft.txt");
            assert_eq!(file_name(&slash), "back\\slash.txt");
        }

        assert_eq!(
            file_name(&temp_copy_path(AREA, &key, "a/b").unwrap()),
            "a_b"
        );
        assert_eq!(
            file_name(&temp_copy_path(AREA, &key, "x\0y").unwrap()),
            "x_y"
        );
        let empty = temp_copy_path(AREA, &key, "").unwrap();
        assert_eq!(file_name(&empty), "file");
        assert_eq!(temp_copy_path(AREA, &key, ".").unwrap(), empty);
        assert_eq!(temp_copy_path(AREA, &key, "..").unwrap(), empty);

        let other = uuid::Uuid::new_v4().to_string();
        let same_name = temp_copy_path(AREA, &other, ".env").unwrap();
        assert_ne!(env.parent(), same_name.parent());
        assert_eq!(temp_copy_path(AREA, &key, ".env").unwrap(), env);

        let _ = std::fs::remove_dir_all(env.parent().unwrap());
        let _ = std::fs::remove_dir_all(same_name.parent().unwrap());
    }

    #[test]
    fn temp_copy_path_refuses_a_key_that_is_not_one_folder_name() {
        for key in ["", ".", "..", "a/b", "../x", "/abs"] {
            assert!(
                temp_copy_path(AREA, key, "file.txt").is_err(),
                "key {key:?} was accepted"
            );
        }
    }

    #[test]
    fn remove_temp_copy_removes_the_file_and_its_folder() {
        let path = temp_copy_path(AREA, &uuid::Uuid::new_v4().to_string(), "hello.txt").unwrap();
        let folder = path.parent().unwrap().to_path_buf();
        std::fs::write(&path, b"hi").unwrap();
        remove_temp_copy(&path).unwrap();
        assert!(!path.exists());
        assert!(!folder.exists());
    }

    #[test]
    fn remove_temp_copy_removes_the_folder_when_the_file_was_never_made() {
        let path = temp_copy_path(AREA, &uuid::Uuid::new_v4().to_string(), "missing.txt").unwrap();
        let folder = path.parent().unwrap().to_path_buf();
        assert!(!path.exists());
        assert!(folder.is_dir());
        remove_temp_copy(&path).unwrap();
        assert!(!folder.exists());
    }

    #[cfg(windows)]
    #[test]
    fn temp_copy_path_replaces_windows_only_characters() {
        let key = uuid::Uuid::new_v4().to_string();
        let cases = [
            ("a:b*c?.txt", "a_b_c_.txt"),
            ("name.", "name_"),
            ("CON", "_CON"),
            ("con.txt", "_con.txt"),
            ("COM1", "_COM1"),
            ("LPT²", "_LPT²"),
            ("console.txt", "console.txt"),
            ("COM0", "COM0"),
        ];
        for (raw, expected) in cases {
            let path = temp_copy_path(AREA, &key, raw).unwrap();
            assert_eq!(file_name(&path), expected, "{raw}");
        }
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join(AREA).join(&key));
    }
}
