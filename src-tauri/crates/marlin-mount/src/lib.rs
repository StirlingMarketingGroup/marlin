//! One helper process per user that owns every mount.
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
pub use mounts::{Adapter, MountError, Providers, Session};
pub use providers::LinkedProviders;
pub use server::{BindError, Config, Server};
