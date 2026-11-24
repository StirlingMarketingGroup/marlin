//! The wire types and the client the app uses to control the mount helper.

use std::path::PathBuf;

pub mod mount_name;
pub mod protocol;

// A dev build, one with debug assertions, runs a helper of its own with its own socket, spool, and mount folder, so a developer's mounts never mix with those of an installed Marlin (#541).

/// The name of the folder that holds the helper's socket and spool.
const HELPER_FOLDER: &str = if cfg!(debug_assertions) {
    "marlin-dev"
} else {
    "marlin"
};

/// The name of the mount folder in the home folder (decision 0001, choice 5).
const MOUNT_FOLDER: &str = if cfg!(debug_assertions) {
    "Marlin Dev"
} else {
    "Marlin"
};

/// `mount.sock` in the helper's folder under the runtime folder, or under the local data folder when there is none.
///
/// The helper uses this same path, so the app and the helper cannot disagree.
pub fn default_socket() -> Option<PathBuf> {
    let folder = dirs::runtime_dir().or_else(dirs::data_local_dir)?;
    Some(folder.join(HELPER_FOLDER).join("mount.sock"))
}

/// The helper's mount folder: `~/Marlin`, or `~/Marlin Dev` in a dev build.
pub fn default_root() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(MOUNT_FOLDER))
}

/// `spool` in the helper's folder under the data folder. A data folder, never a cache folder (decision 0005, choice 10). On macOS a release build's folder also holds the app's own files and the socket.
pub fn default_spool() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join(HELPER_FOLDER).join("spool"))
}

#[cfg(unix)]
mod client;

#[cfg(unix)]
pub use client::{ask_to_stop, shutdown, Client, ClientError, Event, Start};

#[cfg(all(test, debug_assertions))]
mod tests {
    use std::path::Path;

    #[test]
    fn a_dev_build_has_paths_of_its_own() {
        let socket = super::default_socket().unwrap();
        assert!(socket.ends_with(Path::new("marlin-dev").join("mount.sock")));
        assert_eq!(
            super::default_root().unwrap().file_name().unwrap(),
            "Marlin Dev"
        );
        assert!(super::default_spool()
            .unwrap()
            .ends_with(Path::new("marlin-dev").join("spool")));
    }
}
