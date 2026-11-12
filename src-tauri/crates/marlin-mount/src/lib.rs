//! One helper process per user that owns every mount.
//!
//! The app talks to it over a Unix socket with JSON-RPC 2.0. OS adapters and storage providers plug into the traits in `mounts`.

#![cfg(unix)]

pub mod log;
mod mounts;
pub mod protocol;
mod server;

pub use mounts::{Adapter, MountError, Providers, Session};
pub use server::{BindError, Config, Server};
