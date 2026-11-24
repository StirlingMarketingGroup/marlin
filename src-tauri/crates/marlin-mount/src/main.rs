//! Mount helper binary. One process per socket, spoken to over that Unix socket.

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

    use marlin_mount::{log, BindError, Config, LinkedProviders, OsAdapter, Server};
    use tokio::signal::unix::{signal, SignalKind};

    let mut socket = None;
    let mut root = None;
    let mut spool = None;
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
            "--spool" => {
                let Some(value) = args.next() else {
                    usage();
                };
                spool = Some(PathBuf::from(value));
            }
            _ => usage(),
        }
    }

    let socket = socket.or_else(marlin_mount_client::default_socket);
    let root = root.or_else(marlin_mount_client::default_root);
    let spool = spool.or_else(marlin_mount_client::default_spool);
    let (Some(socket), Some(root), Some(spool)) = (socket, root, spool) else {
        log::write("error", "helper.no_home_folder", serde_json::Value::Null);
        std::process::exit(1);
    };

    let server = match Server::bind(
        Config {
            socket,
            root,
            spool,
            options: marlin_vfs::Options::default(),
        },
        Arc::new(OsAdapter),
        Arc::new(LinkedProviders::new(
            marlin_gdrive::Config::default(),
            marlin_gdrive::OAuthClient::load,
        )),
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
    eprintln!("usage: marlin-mount [--socket <path>] [--root <path>] [--spool <path>]");
    std::process::exit(2);
}
