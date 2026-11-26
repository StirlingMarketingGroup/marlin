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
    /// JSON-RPC standard: Method not found
    pub const METHOD_NOT_FOUND: i32 = -32601;
    /// JSON-RPC standard: Invalid params
    pub const INVALID_PARAMS: i32 = -32602;
    /// JSON-RPC standard: Internal error
    pub const INTERNAL_ERROR: i32 = -32603;

    /// SMB authentication failed
    pub const SMB_AUTH_FAILED: i32 = -1002;
    /// SMB path not found
    pub const SMB_PATH_NOT_FOUND: i32 = -1003;
    /// SMB permission denied
    pub const SMB_PERMISSION_DENIED: i32 = -1004;
    /// The `smbclient` program is missing. The text is [`super::SMBCLIENT_MISSING`].
    pub const SMB_LIBRARY_MISSING: i32 = -1005;
    /// The server could not be reached, or the connection broke during the call.
    pub const SMB_CONNECTION_LOST: i32 = -1006;
    /// A file was asked for and the path is a folder.
    pub const SMB_IS_A_FOLDER: i32 = -1007;
    /// A folder was asked for and the path is a file.
    pub const SMB_NOT_A_FOLDER: i32 = -1008;
    /// The name is already taken.
    pub const SMB_ALREADY_EXISTS: i32 = -1009;
    /// A folder to remove or replace holds something.
    pub const SMB_NOT_EMPTY: i32 = -1010;
    /// The file to replace no longer has the size and modified time the caller sent, or its access changed during the upload.
    pub const SMB_CHANGED: i32 = -1011;
    /// The server's disk or the user's quota is full.
    pub const SMB_NO_SPACE: i32 = -1012;
    /// Generic SMB error
    pub const SMB_ERROR: i32 = -1000;
}

/// What `marlin-smb` answers with `SMB_LIBRARY_MISSING`, when a call that runs the `smbclient` program (`list_shares`, `rename`, and `save_file`'s publish) cannot find it.
pub const SMBCLIENT_MISSING: &str = "smbclient not found. Install Samba (macOS: `brew install samba`, Linux: `sudo apt-get install smbclient`).";

/// The most bytes one `read` may ask for.
/// A longer request fails with `INVALID_PARAMS`, so a caller that needs more asks again from where the last read ended.
pub const READ_MAX: u32 = 16 * 1024 * 1024;

/// JSON-RPC 2.0 request.
// No `Debug`: `params` holds the password as plain JSON.
#[derive(Clone, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub params: serde_json::Value,
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
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SmbCredentials {
    pub hostname: String,
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub domain: Option<String>,
}

// Written by hand: a derived `Debug` would print the password, and every `*Params` type prints its credentials.
impl std::fmt::Debug for SmbCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmbCredentials")
            .field("hostname", &self.hostname)
            .field("username", &self.username)
            .field("password", &"<hidden>")
            .field("domain", &self.domain)
            .finish()
    }
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
    /// When false, a taken name answers `SMB_ALREADY_EXISTS` and nothing changes. When true, the item at `to_path` is replaced in one step. The app sends no `replace` field, so it gets false.
    #[serde(default)]
    pub replace: bool,
}

/// The size and modified time of a file to replace.
/// `modified` is whole seconds since 1970, as a `get_file_metadata` answer's time gives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStamp {
    pub size: u64,
    pub modified: i64,
}

/// Parameters for save_file.
///
/// A call that ends without its answer (the caller stopped it, or the process died), or that answers an error after its `staged` notice, may have published the file or not: read the file's stamp again before the next replace. An empty file sends no `progress` notice.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveFileParams {
    pub credentials: SmbCredentials,
    pub share: String,
    /// The local file to send.
    pub source_path: String,
    /// Where the file ends up.
    pub path: String,
    /// A free name in the same folder as `path`, where the bytes go first. The caller picks it, so it can remove the file after it stops `marlin-smb` in the middle (see [`Notice::Staged`]). Pick a new name for each call: a taken one answers `SMB_ALREADY_EXISTS`, as a taken `path` does.
    pub temporary_path: String,
    /// The modified time to give the file, RFC 3339. A server may refuse to set it, so the next `FileStamp` comes from the answer's time.
    pub modified: String,
    /// `None` makes a new file. `Some` replaces the file at `path`, which must still have this size and modified time.
    pub replaces: Option<FileStamp>,
}

/// What a long call sends before its answer, as a JSON-RPC notification: `{"jsonrpc":"2.0","method":"staged","params":{"id":7}}`. A notification has no `id` of its own and gets no answer, so each names the request it reports on in its params.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum Notice {
    /// `save_file` made its temporary file. From this notice on, a caller that stops the call owns that file's removal, and when the removal fails, as with no connection, it tries again later. Before it, the name may belong to someone else, and the caller must not remove it. When `marlin-smb` is stopped after it made the file but before this notice arrives, the file stays on the server: nothing knows it is there.
    Staged { id: u64 },
    /// The bytes `save_file` has sent from the local file so far.
    Progress { id: u64, bytes: u64 },
}

impl Notice {
    /// The request this notice reports on.
    pub fn id(&self) -> u64 {
        match self {
            Notice::Staged { id } | Notice::Progress { id, .. } => *id,
        }
    }
}

/// One notice as its line holds it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoticeLine {
    pub jsonrpc: String,
    #[serde(flatten)]
    pub notice: Notice,
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

/// Parameters for read method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadParams {
    pub credentials: SmbCredentials,
    pub share: String,
    pub path: String,
    pub offset: u64,
    pub length: u32,
}

/// Bytes from one `read`.
/// The bytes travel as standard base64 text, because a JSON string cannot hold raw bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadResult {
    #[serde(with = "base64_text")]
    pub data: Vec<u8>,
}

mod base64_text {
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(deserializer)?;
        base64::engine::general_purpose::STANDARD
            .decode(text)
            .map_err(serde::de::Error::custom)
    }
}

/// Parameters for space method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpaceParams {
    pub credentials: SmbCredentials,
    pub share: String,
}

/// A share's total and free bytes.
/// `space` answers `None`, which is JSON `null`, when the server reports no size.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpaceResult {
    pub total: u64,
    pub free: u64,
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

/// Parameters for upload_file method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadFileParams {
    pub credentials: SmbCredentials,
    pub share: String,
    /// Local path to read the file from.
    pub source_path: String,
    /// SMB path to write the file to.
    pub dest_path: String,
}

/// Result of upload_file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadFileResult {
    pub size: u64,
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

/// Parameters for download_partial method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadPartialParams {
    pub credentials: SmbCredentials,
    pub share: String,
    pub path: String,
    /// Local path to write the file to.
    pub dest_path: String,
    /// Maximum number of bytes to download.
    pub max_bytes: u64,
}

/// Result of download_partial.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadPartialResult {
    pub path: String,
    pub bytes_written: u64,
    pub total_size: u64,
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
    pub const READ: &str = "read";
    pub const SPACE: &str = "space";
    pub const DOWNLOAD_FILE: &str = "download_file";
    pub const DOWNLOAD_PARTIAL: &str = "download_partial";
    pub const UPLOAD_FILE: &str = "upload_file";
    pub const SAVE_FILE: &str = "save_file";
}
