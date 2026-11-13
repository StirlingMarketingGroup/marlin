//! One Google account as a read-only [`marlin_vfs::Provider`] for a native mount.
//!
//! Access tokens come from [`AccessTokens`]. This crate does not sign in, read a keychain, or talk to the mount helper.

mod api;
mod provider;

pub use provider::{AccessTokens, Config, DriveProvider};
