//! Mount helper binary. One process per socket, spoken to over that Unix socket.

#[cfg(all(feature = "app-test", not(debug_assertions)))]
compile_error!("The `app-test` feature points the helper at a test keychain and a fake Google. Build it in debug only.");

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
    use std::time::Duration;

    use marlin_mount::{log, BindError, Config, LinkedProviders, OsAdapter, Server, SmbConfig};
    use tokio::signal::unix::{signal, SignalKind};

    // The test build keeps secrets in the test keychain it shares with the test app, never the real one.
    #[cfg(feature = "app-test")]
    marlin_test_keychain::install_for_app_test();

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
    let servers = marlin_sftp::servers_path().ok();
    let known_hosts = marlin_sftp::known_hosts_path().ok();
    let smb_servers = marlin_smb_client::servers_path().ok();
    let (
        Some(socket),
        Some(root),
        Some(spool),
        Some(servers),
        Some(known_hosts),
        Some(smb_servers),
    ) = (socket, root, spool, servers, known_hosts, smb_servers)
    else {
        log::write("error", "helper.no_home_folder", serde_json::Value::Null);
        std::process::exit(1);
    };

    // The test build talks to the fake Google only.
    #[cfg(feature = "app-test")]
    let (drive, oauth_client) = (
        marlin_gdrive::app_test::drive_config(),
        marlin_gdrive::app_test::oauth_client,
    );
    #[cfg(not(feature = "app-test"))]
    let (drive, oauth_client) = (
        marlin_gdrive::Config::default(),
        marlin_gdrive::OAuthClient::load,
    );

    let server = match Server::bind(
        Config {
            socket,
            root,
            spool,
            options: marlin_vfs::Options::default(),
        },
        Arc::new(OsAdapter),
        Arc::new(LinkedProviders::new(
            drive,
            oauth_client,
            marlin_sftp::Config {
                servers,
                known_hosts,
                // One minute: often enough to see a fix made on the server, rarely enough that OpenSSH does not penalize the address.
                retry_after: Duration::from_secs(60),
            },
            SmbConfig {
                // Only the file beside the helper, never `PATH` (decision 0006).
                binary: std::env::current_exe()
                    .ok()
                    .map(|exe| exe.with_file_name("marlin-smb")),
                servers: smb_servers,
            },
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
