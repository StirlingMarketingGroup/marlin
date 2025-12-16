use async_trait::async_trait;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, RwLock};

use crate::fs_utils::FileItem;

mod file;
pub mod gdrive;

pub use file::FileSystemProvider;
pub use gdrive::GoogleDriveProvider;

pub type ProviderRef = Arc<dyn LocationProvider + Send + Sync>;

type ProviderMap = HashMap<String, ProviderRef>;

static REGISTRY: Lazy<RwLock<ProviderMap>> = Lazy::new(|| {
    let mut map = HashMap::new();
    let file_provider: ProviderRef = Arc::new(FileSystemProvider::default());
    map.insert(file_provider.scheme().to_string(), file_provider);
    let gdrive_provider: ProviderRef = Arc::new(GoogleDriveProvider::default());
    map.insert(gdrive_provider.scheme().to_string(), gdrive_provider);
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

#[async_trait]
pub trait LocationProvider: Send + Sync {
    fn scheme(&self) -> &'static str;
    fn capabilities(&self, location: &Location) -> LocationCapabilities;

    async fn read_directory(&self, location: &Location) -> Result<ProviderDirectoryEntries, String>;
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
