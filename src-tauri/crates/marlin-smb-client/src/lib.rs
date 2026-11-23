//! The SMB helper protocol, the client that runs `marlin-smb`, and the saved SMB servers.
//! The crate also holds the read-only mount provider for one saved server.
//! The app links this crate. `marlin-smb` links the protocol only.

pub mod protocol;

#[cfg(feature = "client")]
mod provider;
#[cfg(feature = "client")]
mod servers;
#[cfg(feature = "client")]
mod sidecar;

#[cfg(feature = "client")]
pub use provider::{SmbProvider, SmbSignIn, PROCESSES};
#[cfg(feature = "client")]
pub use servers::{
    add_server, migrate_plaintext_passwords, remove_server, saved_servers, server_credentials,
    servers_path, CredentialsError, SmbServerCredentials, SmbServerInfo,
};
#[cfg(feature = "client")]
pub use sidecar::{CallError, Sidecar, StartError};
