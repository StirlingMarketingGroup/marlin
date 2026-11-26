//! JSON-RPC 2.0 wire types for the mount helper.
//!
//! Framing matches `marlin-smb`: one JSON object per line.

use serde::{Deserialize, Serialize};

/// JSON-RPC 2.0 version string.
pub const JSONRPC_VERSION: &str = "2.0";

/// Protocol number the helper and the app speak. Raised when the control API changes.
pub const PROTOCOL_VERSION: u32 = 11;

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
    pub const DEAD_MOUNT: i32 = -1012;
    pub const SIGN_IN_NEEDED: i32 = -1013;
    pub const UNREACHABLE: i32 = -1014;
    /// That file has no unsent bytes in this mount. Also for a mount id that is not the mount's current one.
    pub const NOT_UNSENT: i32 = -1015;
    /// A discard waited too long for the mount's other changes.
    pub const BUSY: i32 = -1016;
    /// The mount folder holds things the helper did not put there, and no marker. The helper does not mount in it.
    pub const MOUNT_FOLDER_TAKEN: i32 = -1017;
    /// The account has no mount that is up: none, or one that is starting, unmounting, or failed.
    pub const NOT_MOUNTED: i32 = -1018;
    /// The mount shows no item at that path, or the account has no item with that ID.
    pub const NO_SUCH_ITEM: i32 = -1019;
    /// An item already holds the name a `copy` would give its copy.
    pub const NAME_TAKEN: i32 = -1020;
}

/// Control methods and the notifications the helper sends.
pub mod methods {
    pub const HELLO: &str = "hello";
    pub const MOUNT: &str = "mount";
    pub const UNMOUNT: &str = "unmount";
    pub const LIST: &str = "list";
    pub const STATUS: &str = "status";
    pub const SHUTDOWN: &str = "shutdown";
    pub const DEAD_MOUNTS: &str = "deadMounts";
    pub const UNSENT: &str = "unsent";
    pub const RETRY_UNSENT: &str = "retryUnsent";
    pub const DISCARD_UNSENT: &str = "discardUnsent";
    pub const LOCATE: &str = "locate";
    pub const IDENTIFY: &str = "identify";
    pub const REFRESH: &str = "refresh";
    pub const THUMBNAIL: &str = "thumbnail";
    pub const COPY: &str = "copy";
    pub const MOUNT_CHANGED: &str = "mountChanged";
    pub const UNSENT_CHANGED: &str = "unsentChanged";
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

/// Where a mount is in its life. A mount that is up is `Mounted` when its remote answers, or `Degraded`, `AuthRequired`, or `Offline` when it does not. `Unmounted` appears only in the last `mountChanged` for a mount.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MountState {
    Starting,
    Mounted,
    Degraded,
    AuthRequired,
    Offline,
    Unmounting,
    Failed,
    Unmounted,
}

impl MountState {
    /// Whether the mount is up: the OS serves its folder, whatever its remote's health. Decision 0009, choice 4, names the same four states.
    pub fn is_up(self) -> bool {
        matches!(
            self,
            Self::Mounted | Self::Degraded | Self::Offline | Self::AuthRequired
        )
    }
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
    /// Why the mount takes no writes. Absent when it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only: Option<ReadOnly>,
}

/// Why a mount takes no writes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum ReadOnly {
    /// The provider has no write calls.
    Provider,
    /// The mount's spool folder could not be made or read. `message` says why.
    Spool { message: String },
}

/// The files of one mount that still have to reach the remote. The answer to `unsent`, and the body of `unsentChanged`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnsentFiles {
    pub provider: String,
    pub account: String,
    /// Which mount attempt this list belongs to: a UUID made when the attempt starts. `retryUnsent` and `discardUnsent` send it back.
    pub mount_id: String,
    pub files: Vec<UnsentFile>,
    /// Spool files from an earlier run that have no readable record. They are kept and never uploaded. The user can open them at these paths.
    pub unplaced: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnsentFile {
    /// The core's node number. Only means something together with `mount_id`.
    pub node: u64,
    /// The file's name as the remote gets it. `path` holds its folder, when known.
    pub name: String,
    /// The file's path in the mount, as `locate` answers one. Missing when the mount no longer holds a folder on the way, as for a file recovered from an earlier run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub size: u64,
    #[serde(flatten)]
    pub state: UnsentState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum UnsentState {
    Open,
    Waiting,
    Uploading { sent: u64 },
    Failed { error: String },
    Refused { error: String },
}

/// Body of `retryUnsent` and `discardUnsent`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnsentParams {
    pub provider: String,
    pub account: String,
    pub mount_id: String,
    pub node: u64,
}

/// Body of a `locate` request. `ref` is the remote's own ID for an item, such as the file ID in a Google Drive link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocateParams {
    pub provider: String,
    pub account: String,
    #[serde(rename = "ref")]
    pub reference: String,
}

/// Body of an `identify` request. `path` is mount-relative: names as the mount shows them, joined with `/`. The empty path is the top of the mount.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentifyParams {
    pub provider: String,
    pub account: String,
    pub path: String,
}

/// The answer to `identify`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "item", rename_all = "kebab-case")]
pub enum Identified {
    /// The remote has the item. `ref` is the remote's own ID for it, absent when it has none: the top of the mount, and a folder only the mount shows.
    Remote {
        #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
        reference: Option<String>,
    },
    /// A file that is only in the spool. It has not reached the remote yet.
    New,
}

/// Body of a `thumbnail` request. `path` is mount-relative, as for `identify`. `size` is the longer side the caller asks for, in pixels; the remote may send another size. `file` is an absolute path that must not exist yet: the helper makes the file there, with mode 0600, and writes the picture into it. It never replaces what is already at that path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThumbnailParams {
    pub provider: String,
    pub account: String,
    pub path: String,
    pub size: u32,
    pub file: String,
}

/// A width and a height in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dimensions {
    pub width: u32,
    pub height: u32,
}

/// The answer to `thumbnail`. `Written` means the helper made the request's `file` and finished writing the picture into it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "thumbnail", rename_all = "kebab-case")]
pub enum Thumbnailed {
    /// The helper wrote the remote's picture of the file to the request's `file`. `dimensions` is the file's own width and height, absent when the remote does not know them. It is never the picture's size.
    Written {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dimensions: Option<Dimensions>,
    },
    /// The remote has no picture of the item.
    NoPicture,
    /// The item may not use a provider call (decision 0009, choice 7): the remote does not have the file's current bytes, so its picture would be wrong.
    NotEligible,
}

/// Body of a `copy` request. `from` and `to` are mount-relative paths, read as for `identify`. `to` ends with the copy's name, which must be free.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyParams {
    pub provider: String,
    pub account: String,
    pub from: String,
    pub to: String,
}

/// The answer to `copy`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "copy", rename_all = "kebab-case")]
pub enum Copied {
    /// The remote made the copy. `path` is where the mount shows it, mount-relative.
    Copied { path: String },
    /// The copy may not use the provider's copy (decision 0009, choice 7), and nothing changed. The caller copies through the filesystem.
    NotEligible,
}

/// A dead mount under the mount folder: its server was killed, and the OS still lists it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeadMount {
    pub name: String,
    pub mount_point: String,
}

#[cfg(test)]
mod tests {
    use super::{UnsentFile, UnsentFiles, UnsentState};

    #[test]
    fn unsent_files_keep_their_shape_on_the_wire() {
        let files = UnsentFiles {
            provider: "gdrive".into(),
            account: "ana@example.com".into(),
            mount_id: "00000000-0000-4000-8000-000000000000".into(),
            files: [
                UnsentState::Open,
                UnsentState::Waiting,
                UnsentState::Uploading { sent: 3 },
                UnsentState::Failed {
                    error: "offline".into(),
                },
                UnsentState::Refused {
                    error: "access denied".into(),
                },
            ]
            .into_iter()
            .enumerate()
            .map(|(node, state)| UnsentFile {
                node: node as u64,
                name: "a.txt".into(),
                path: (node == 0).then(|| "My Drive/a.txt".into()),
                size: 5,
                state,
            })
            .collect(),
            unplaced: vec!["/spool/7".into()],
        };
        let wire = serde_json::to_value(&files).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({
                "provider": "gdrive",
                "account": "ana@example.com",
                "mountId": "00000000-0000-4000-8000-000000000000",
                "files": [
                    {"node": 0, "name": "a.txt", "path": "My Drive/a.txt", "size": 5, "state": "open"},
                    {"node": 1, "name": "a.txt", "size": 5, "state": "waiting"},
                    {"node": 2, "name": "a.txt", "size": 5, "state": "uploading", "sent": 3},
                    {"node": 3, "name": "a.txt", "size": 5, "state": "failed", "error": "offline"},
                    {"node": 4, "name": "a.txt", "size": 5, "state": "refused", "error": "access denied"},
                ],
                "unplaced": ["/spool/7"],
            })
        );
        assert_eq!(serde_json::from_value::<UnsentFiles>(wire).unwrap(), files);
    }

    #[test]
    fn identified_and_locate_keep_their_shape_on_the_wire() {
        use super::{Identified, LocateParams};

        let remote = Identified::Remote {
            reference: Some("abc".into()),
        };
        let wire = serde_json::to_value(&remote).unwrap();
        assert_eq!(wire, serde_json::json!({"item": "remote", "ref": "abc"}));
        assert_eq!(serde_json::from_value::<Identified>(wire).unwrap(), remote);

        let bare = Identified::Remote { reference: None };
        let wire = serde_json::to_value(&bare).unwrap();
        assert_eq!(wire, serde_json::json!({"item": "remote"}));
        assert_eq!(serde_json::from_value::<Identified>(wire).unwrap(), bare);

        let new = Identified::New;
        let wire = serde_json::to_value(&new).unwrap();
        assert_eq!(wire, serde_json::json!({"item": "new"}));
        assert_eq!(serde_json::from_value::<Identified>(wire).unwrap(), new);

        let params = LocateParams {
            provider: "gdrive".into(),
            account: "ana".into(),
            reference: "abc".into(),
        };
        let wire = serde_json::to_value(&params).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({"provider": "gdrive", "account": "ana", "ref": "abc"})
        );
        assert_eq!(
            serde_json::from_value::<LocateParams>(wire).unwrap(),
            params
        );
    }

    #[test]
    fn thumbnail_keeps_its_shape_on_the_wire() {
        use super::{Dimensions, ThumbnailParams, Thumbnailed};

        let written = Thumbnailed::Written {
            dimensions: Some(Dimensions {
                width: 4000,
                height: 3000,
            }),
        };
        let wire = serde_json::to_value(&written).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({"thumbnail": "written", "dimensions": {"width": 4000, "height": 3000}})
        );
        assert_eq!(
            serde_json::from_value::<Thumbnailed>(wire).unwrap(),
            written
        );

        let bare = Thumbnailed::Written { dimensions: None };
        let wire = serde_json::to_value(&bare).unwrap();
        assert_eq!(wire, serde_json::json!({"thumbnail": "written"}));
        assert_eq!(serde_json::from_value::<Thumbnailed>(wire).unwrap(), bare);

        let none = Thumbnailed::NoPicture;
        let wire = serde_json::to_value(&none).unwrap();
        assert_eq!(wire, serde_json::json!({"thumbnail": "no-picture"}));
        assert_eq!(serde_json::from_value::<Thumbnailed>(wire).unwrap(), none);

        let not_eligible = Thumbnailed::NotEligible;
        let wire = serde_json::to_value(&not_eligible).unwrap();
        assert_eq!(wire, serde_json::json!({"thumbnail": "not-eligible"}));
        assert_eq!(
            serde_json::from_value::<Thumbnailed>(wire).unwrap(),
            not_eligible
        );

        let params = ThumbnailParams {
            provider: "gdrive".into(),
            account: "ana".into(),
            path: "My Drive/photo.jpg".into(),
            size: 256,
            file: "/tmp/picture".into(),
        };
        let wire = serde_json::to_value(&params).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({
                "provider": "gdrive",
                "account": "ana",
                "path": "My Drive/photo.jpg",
                "size": 256,
                "file": "/tmp/picture",
            })
        );
        assert_eq!(
            serde_json::from_value::<ThumbnailParams>(wire).unwrap(),
            params
        );
    }

    #[test]
    fn copy_keeps_its_shape_on_the_wire() {
        use super::{Copied, CopyParams};

        let copied = Copied::Copied {
            path: "Projects/a copy.txt".into(),
        };
        let wire = serde_json::to_value(&copied).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({"copy": "copied", "path": "Projects/a copy.txt"})
        );
        assert_eq!(serde_json::from_value::<Copied>(wire).unwrap(), copied);

        let not_eligible = Copied::NotEligible;
        let wire = serde_json::to_value(&not_eligible).unwrap();
        assert_eq!(wire, serde_json::json!({"copy": "not-eligible"}));
        assert_eq!(
            serde_json::from_value::<Copied>(wire).unwrap(),
            not_eligible
        );

        let params = CopyParams {
            provider: "gdrive".into(),
            account: "ana".into(),
            from: "Projects/a.txt".into(),
            to: "Projects/a copy.txt".into(),
        };
        let wire = serde_json::to_value(&params).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({
                "provider": "gdrive",
                "account": "ana",
                "from": "Projects/a.txt",
                "to": "Projects/a copy.txt",
            })
        );
        assert_eq!(serde_json::from_value::<CopyParams>(wire).unwrap(), params);
    }
}
