//! One helper process per socket that owns every mount in its mount folder.
//!
//! The app talks to it over a Unix socket with JSON-RPC 2.0. OS adapters and storage providers plug into the traits in `mounts`.

#![cfg(unix)]

mod adapter;
pub mod log;
mod mounts;
mod providers;
pub use marlin_mount_client::protocol;
mod server;

pub use adapter::OsAdapter;
pub use mounts::{Adapter, MountError, Opened, Providers, Session, MOUNT_FOLDER_MARKER};
pub use providers::LinkedProviders;
pub use server::{BindError, Config, Server};
