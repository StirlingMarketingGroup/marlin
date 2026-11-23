//! Saved SFTP servers, the check of a server's host key, and the connection.
//! The app links this crate.

mod connect;
mod host_key;
mod servers;

pub use connect::{connect, ConnectError};
pub use host_key::{known_hosts_path, trust_host_key};
pub use servers::{
    add_server, remove_server, saved_servers, server_credentials, servers_path, CredentialsError,
    Passphrase, SftpServerCredentials, SftpServerInfo, SignIn,
};
