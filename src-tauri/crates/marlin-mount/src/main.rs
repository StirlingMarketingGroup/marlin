//! Mount helper binary. One process per user, spoken to over a Unix socket.

/// The mount adapter this OS has: FUSE through `marlin-fuse`.
#[cfg(target_os = "linux")]
mod adapter {
    use std::path::Path;
    use std::sync::Arc;

    use marlin_mount::{Adapter, MountError, Session};
    use marlin_vfs::Vfs;

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
}

/// This OS has no mount adapter yet (macOS is #373), so every mount is refused.
#[cfg(all(unix, not(target_os = "linux")))]
mod adapter {
    use std::path::Path;
    use std::sync::Arc;

    use marlin_mount::{Adapter, MountError, Session};
    use marlin_vfs::Vfs;

    pub struct OsAdapter;

    #[async_trait::async_trait]
    impl Adapter for OsAdapter {
        async fn mount(
            &self,
            _vfs: Arc<Vfs>,
            _mount_point: &Path,
        ) -> Result<Box<dyn Session>, MountError> {
            Err(MountError::Adapter(
                "Marlin cannot mount on this OS yet.".into(),
            ))
        }
    }
}

#[cfg(not(unix))]
fn main() {
    eprintln!("marlin-mount does not run on this OS yet (issue #375)");
    std::process::exit(1);
}

#[cfg(unix)]
#[tokio::main]
async fn main() {
    use std::path::PathBuf;
    use std::sync::Arc;

    use marlin_mount::{log, BindError, Config, MountError, Providers, Server};
    use marlin_vfs::Provider;
    use tokio::signal::unix::{signal, SignalKind};

    struct NoProviders;

    #[async_trait::async_trait]
    impl Providers for NoProviders {
        async fn open(
            &self,
            _provider: &str,
            _account: &str,
        ) -> Result<Arc<dyn Provider>, MountError> {
            Err(MountError::UnknownProvider)
        }
    }

    let mut socket = None;
    let mut root = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--socket" => {
                let Some(value) = args.next() else {
                    usage();
                };
                socket = Some(PathBuf::from(value));
            }
            "--root" => {
                let Some(value) = args.next() else {
                    usage();
                };
                root = Some(PathBuf::from(value));
            }
            _ => usage(),
        }
    }

    let socket = socket.or_else(|| {
        let folder = dirs::runtime_dir().or_else(dirs::data_local_dir)?;
        Some(folder.join("marlin").join("mount.sock"))
    });
    let root = root.or_else(|| Some(dirs::home_dir()?.join("Marlin")));
    let (Some(socket), Some(root)) = (socket, root) else {
        log::write("error", "helper.no_home_folder", serde_json::Value::Null);
        std::process::exit(1);
    };

    let server = match Server::bind(
        Config { socket, root },
        Arc::new(adapter::OsAdapter),
        Arc::new(NoProviders),
    ) {
        Ok(server) => server,
        // Another helper owns the mounts. A second one must never take them over.
        Err(BindError::AlreadyRunning) => {
            log::write("info", "helper.already_running", serde_json::Value::Null);
            std::process::exit(0);
        }
        Err(error) => {
            log::write(
                "error",
                "helper.bind_failed",
                serde_json::json!({"message": error.to_string()}),
            );
            std::process::exit(1);
        }
    };

    let mut sigterm = signal(SignalKind::terminate()).expect("sigterm");
    let mut sigint = signal(SignalKind::interrupt()).expect("sigint");
    server
        .run(async move {
            tokio::select! {
                _ = sigterm.recv() => {}
                _ = sigint.recv() => {}
            }
        })
        .await;
}

#[cfg(unix)]
fn usage() -> ! {
    eprintln!("usage: marlin-mount [--socket <path>] [--root <path>]");
    std::process::exit(2);
}
