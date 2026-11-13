//! The wire types and the client the app uses to control the mount helper.

use std::path::PathBuf;

pub mod protocol;

/// `marlin/mount.sock` under the runtime folder, or the local data folder when there is none.
///
/// The helper uses this same path, so the app and the helper cannot disagree.
pub fn default_socket() -> Option<PathBuf> {
    let folder = dirs::runtime_dir().or_else(dirs::data_local_dir)?;
    Some(folder.join("marlin").join("mount.sock"))
}

#[cfg(unix)]
mod client;

#[cfg(unix)]
pub use client::{shutdown, Client, ClientError, Event, Start};
