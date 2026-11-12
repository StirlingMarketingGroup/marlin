//! JSON-RPC 2.0 wire types for the mount helper.
//!
//! Framing matches `marlin-smb`: one JSON object per line.

use serde::{Deserialize, Serialize};

/// JSON-RPC 2.0 version string.
pub const JSONRPC_VERSION: &str = "2.0";

/// Protocol number this helper speaks. Raised when the control API changes.
pub const PROTOCOL_VERSION: u32 = 1;

/// JSON-RPC and helper error codes.
pub mod error_codes {
    pub const PARSE_ERROR: i32 = -32700;
    pub const METHOD_NOT_FOUND: i32 = -32601;
    pub const INVALID_PARAMS: i32 = -32602;
    pub const INVALID_NAME: i32 = -1001;
    pub const NAME_IN_USE: i32 = -1002;
    pub const MOUNT_POINT_NOT_EMPTY: i32 = -1003;
    pub const UNKNOWN_PROVIDER: i32 = -1004;
    pub const PROVIDER: i32 = -1005;
    pub const ADAPTER: i32 = -1006;
    pub const UNMOUNT: i32 = -1007;
    pub const IO: i32 = -1008;
    pub const SHUTTING_DOWN: i32 = -1009;
    pub const HELLO_REQUIRED: i32 = -1010;
    pub const PROTOCOL_MISMATCH: i32 = -1011;
}

/// Control methods and the one notification the helper sends.
pub mod methods {
    pub const HELLO: &str = "hello";
    pub const MOUNT: &str = "mount";
    pub const UNMOUNT: &str = "unmount";
    pub const LIST: &str = "list";
    pub const STATUS: &str = "status";
    pub const SHUTDOWN: &str = "shutdown";
    pub const MOUNT_CHANGED: &str = "mountChanged";
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
            }),
        }
    }
}

/// JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
}

/// Body of a `hello` request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloParams {
    pub protocol: u32,
}

/// Body of a successful `hello`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloResult {
    pub protocol: u32,
    pub version: String,
}

/// Body of a `mount` request. `name` is one folder name under the helper's root.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MountParams {
    pub provider: String,
    pub account: String,
    pub name: String,
}

/// Body of an `unmount` or `status` request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountParams {
    pub provider: String,
    pub account: String,
}

/// Where a mount is in its life. `Unmounted` appears only in the last `mountChanged` for a mount.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MountState {
    Starting,
    Mounted,
    Unmounting,
    Failed,
    Unmounted,
}

/// One mount as the helper reports it. `error` is set only when the state is `failed`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MountStatus {
    pub provider: String,
    pub account: String,
    pub name: String,
    pub mount_point: String,
    pub state: MountState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}
