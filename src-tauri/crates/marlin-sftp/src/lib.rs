//! Saved SFTP servers, the check of a server's host key, the connection, and the read-only provider for mounts.
//! The app links this crate, and so does the mount helper, for SFTP mounts.

mod connect;
mod host_key;
mod provider;
mod servers;

pub use connect::{connect, ConnectError};
pub use host_key::{known_hosts_path, trust_host_key};
pub use provider::{Config, SftpAccount, SftpProvider, MAX_CALLS};
pub use servers::{
    add_server, parse_authority, parse_host, remove_server, saved_server, saved_servers,
    server_credentials, servers_path, CredentialsError, Passphrase, SftpServerCredentials,
    SftpServerInfo, SignIn,
};
