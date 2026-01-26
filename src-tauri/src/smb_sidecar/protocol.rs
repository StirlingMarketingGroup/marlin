//! JSON-RPC 2.0 protocol types for SMB sidecar communication.
//!
//! The sidecar uses NDJSON (Newline-Delimited JSON) framing:
//! one JSON object per line, terminated with `\n`.

use serde::{Deserialize, Serialize};

/// JSON-RPC 2.0 version string.
pub const JSONRPC_VERSION: &str = "2.0";

/// SMB-specific error codes.
pub mod error_codes {
    /// JSON-RPC standard: Parse error
    pub const PARSE_ERROR: i32 = -32700;
    /// JSON-RPC standard: Invalid request
    pub const INVALID_REQUEST: i32 = -32600;
    /// JSON-RPC standard: Method not found
    pub const METHOD_NOT_FOUND: i32 = -32601;
    /// JSON-RPC standard: Invalid params
    pub const INVALID_PARAMS: i32 = -32602;
    /// JSON-RPC standard: Internal error
    pub const INTERNAL_ERROR: i32 = -32603;

    /// SMB connection failed
    pub const SMB_CONNECTION_FAILED: i32 = -1001;
    /// SMB authentication failed
    pub const SMB_AUTH_FAILED: i32 = -1002;
    /// SMB path not found
    pub const SMB_PATH_NOT_FOUND: i32 = -1003;
    /// SMB permission denied
    pub const SMB_PERMISSION_DENIED: i32 = -1004;
    /// libsmbclient library missing
    pub const SMB_LIBRARY_MISSING: i32 = -1005;
    /// Operation timeout
    pub const SMB_TIMEOUT: i32 = -1006;
    /// Generic SMB error
    pub const SMB_ERROR: i32 = -1000;
}

/// JSON-RPC 2.0 request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub params: serde_json::Value,
}

impl Request {
    pub fn new(id: u64, method: impl Into<String>, params: impl Serialize) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            method: method.into(),
            params: serde_json::to_value(params).unwrap_or(serde_json::Value::Null),
        }
    }
}

/// JSON-RPC 2.0 response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl Response {
    pub fn success(id: u64, result: impl Serialize) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            result: Some(serde_json::to_value(result).unwrap_or(serde_json::Value::Null)),
            error: None,
        }
    }

    pub fn error(id: u64, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }

    pub fn error_with_data(
        id: u64,
        code: i32,
        message: impl Into<String>,
        data: serde_json::Value,
    ) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
                data: Some(data),
            }),
        }
    }
}

/// JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// SMB credentials passed with each request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmbCredentials {
    pub hostname: String,
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub domain: Option<String>,
}

/// Parameters for read_directory method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadDirectoryParams {
    pub credentials: SmbCredentials,
    pub share: String,
    pub path: String,
}

/// Parameters for get_file_metadata method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetFileMetadataParams {
    pub credentials: SmbCredentials,
    pub share: String,
    pub path: String,
}

/// Parameters for create_directory method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateDirectoryParams {
    pub credentials: SmbCredentials,
    pub share: String,
    pub path: String,
}

/// Parameters for delete method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteParams {
    pub credentials: SmbCredentials,
    pub share: String,
    pub path: String,
}

/// Parameters for rename method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameParams {
    pub credentials: SmbCredentials,
    pub share: String,
    pub from_path: String,
    pub to_path: String,
}

/// Parameters for copy method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopyParams {
    pub credentials: SmbCredentials,
    pub share: String,
    pub from_path: String,
    pub to_path: String,
}

/// Parameters for list_shares method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListSharesParams {
    pub credentials: SmbCredentials,
}

/// Parameters for test_connection method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestConnectionParams {
    pub credentials: SmbCredentials,
}

/// Parameters for download_file method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadFileParams {
    pub credentials: SmbCredentials,
    pub share: String,
    pub path: String,
    /// Local path to write the file to.
    pub dest_path: String,
}

/// A directory entry returned by read_directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectoryEntry {
    pub name: String,
    pub is_directory: bool,
    pub is_hidden: bool,
    pub size: u64,
    /// Modified time as ISO 8601 string.
    pub modified: String,
    pub extension: Option<String>,
}

/// Result of read_directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadDirectoryResult {
    pub entries: Vec<DirectoryEntry>,
}

/// Result of get_file_metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMetadataResult {
    pub name: String,
    pub is_directory: bool,
    pub is_hidden: bool,
    pub size: u64,
    pub modified: String,
    pub extension: Option<String>,
}

/// A share entry returned by list_shares.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareEntry {
    pub name: String,
    pub comment: Option<String>,
}

/// Result of list_shares.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListSharesResult {
    pub shares: Vec<ShareEntry>,
}

/// Result of download_file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadFileResult {
    pub path: String,
    pub size: u64,
}

/// Result of test_connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestConnectionResult {
    pub success: bool,
}

/// Ping result (used to verify sidecar is alive).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PingResult {
    pub version: String,
}

/// SMB method names.
pub mod methods {
    pub const PING: &str = "ping";
    pub const READ_DIRECTORY: &str = "read_directory";
    pub const GET_FILE_METADATA: &str = "get_file_metadata";
    pub const CREATE_DIRECTORY: &str = "create_directory";
    pub const DELETE: &str = "delete";
    pub const RENAME: &str = "rename";
    pub const COPY: &str = "copy";
    pub const LIST_SHARES: &str = "list_shares";
    pub const TEST_CONNECTION: &str = "test_connection";
    pub const DOWNLOAD_FILE: &str = "download_file";
}
