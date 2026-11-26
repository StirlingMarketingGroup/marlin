//! One Google account as a [`marlin_vfs::Provider`] for a native mount.
//!
//! The provider can make a folder, rename or move an item, trash one, and write a file's bytes. `can_write` is true.
//!
//! Access tokens come from [`AccessTokens`]. [`SavedSignIn`] is the one the app and the mount helper share: it reads the sign-in the app saved in the OS keychain. This crate never opens a browser to sign in.

mod api;
mod provider;
mod sign_in;
mod upload;

pub use provider::{AccessTokens, Config, DriveProvider};
pub use sign_in::{keyring_entry, OAuthClient, SavedSignIn, SignInError, KEYRING_SERVICE};

#[cfg(feature = "app-test")]
pub mod app_test;
