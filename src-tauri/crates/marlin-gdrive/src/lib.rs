//! One Google account as a read-only [`marlin_vfs::Provider`] for a native mount.
//!
//! Access tokens come from [`AccessTokens`]. [`SavedSignIn`] is the one the app and the mount helper share: it reads the sign-in the app saved in the OS keychain. This crate never opens a browser to sign in.

mod api;
mod provider;
mod sign_in;

pub use provider::{AccessTokens, Config, DriveProvider};
pub use sign_in::{keyring_entry, OAuthClient, SavedSignIn, SignInError, KEYRING_SERVICE};
