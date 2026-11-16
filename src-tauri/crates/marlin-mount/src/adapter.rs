//! The mount adapter this OS has, as `OsAdapter`.
//!
//! Marlin builds for Linux, macOS, and Windows only. This crate is Unix only, so Linux and macOS are the only targets here, and both mount through `marlin-fuse`.

use std::path::Path;
use std::sync::Arc;

use crate::{Adapter, MountError, Session};
use marlin_vfs::Vfs;

/// The mount adapter this OS has: FUSE through `marlin-fuse`.
pub struct OsAdapter;

#[async_trait::async_trait]
impl Adapter for OsAdapter {
    async fn mount(
        &self,
        vfs: Arc<Vfs>,
        mount_point: &Path,
    ) -> Result<Box<dyn Session>, MountError> {
        match marlin_fuse::mount(vfs, mount_point).await {
            Ok(mount) => Ok(Box::new(FuseSession(mount))),
            Err(error) => Err(MountError::Adapter(error.to_string())),
        }
    }

    async fn is_dead_mount(&self, mount_point: &Path) -> bool {
        marlin_fuse::is_dead_mount(mount_point).await
    }

    async fn unmount_dead(&self, mount_point: &Path) -> Result<(), MountError> {
        marlin_fuse::unmount_dead(mount_point)
            .await
            .map_err(|error| MountError::DeadMount(error.to_string()))
    }
}

struct FuseSession(marlin_fuse::Mount);

#[async_trait::async_trait]
impl Session for FuseSession {
    async fn unmount(&mut self) -> Result<(), MountError> {
        self.0
            .unmount()
            .await
            .map_err(|error| MountError::Unmount(error.to_string()))
    }
}
