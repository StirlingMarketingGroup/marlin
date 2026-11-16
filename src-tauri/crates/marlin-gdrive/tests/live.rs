//! Manual check of one real Google account through [`DriveProvider`].
//!
//! It reads the account named by `MARLIN_GDRIVE_LIVE_ACCOUNT` from the OS keychain, so macOS may
//! ask to allow it. It never changes anything in the account: it only lists and reads.
//!
//! It prints counts, sizes, and times, and never a name, an ID, or file bytes. The assertions on
//! the account's own data print nothing from it either, so the output is safe to paste.
//!
//! Run:
//! `MARLIN_GDRIVE_LIVE_ACCOUNT=<email> cargo test -p marlin-gdrive --test live -- --ignored --nocapture`

use std::sync::Arc;
use std::time::Instant;

use marlin_gdrive::{Config, DriveProvider, OAuthClient, SavedSignIn};
use marlin_vfs::{Kind, Provider};

#[tokio::test]
#[ignore = "reads a real Google account and the real keychain: see the header"]
async fn a_real_account_lists_and_reads() {
    let email = std::env::var("MARLIN_GDRIVE_LIVE_ACCOUNT")
        .expect("Set MARLIN_GDRIVE_LIVE_ACCOUNT to the account's email");
    // The loader's own text can hold a file path, so it stays out of the output.
    let client = OAuthClient::load().unwrap_or_else(|_| panic!("no Google OAuth client"));
    let provider = DriveProvider::new(
        Arc::new(SavedSignIn::new(&email, client)),
        Config::default(),
    );

    let top = provider.list(&provider.root()).await.unwrap();
    let names: Vec<_> = top.iter().map(|item| item.name.as_str()).collect();
    assert_eq!(names, ["My Drive", "Shared drives"]);

    let space = provider.space().await.unwrap();
    assert!(space.total >= space.free);
    println!("space total {} free {}", space.total, space.free);

    let my_drive = top.iter().find(|item| item.name == "My Drive").unwrap();
    let started = Instant::now();
    let items = provider.list(&my_drive.id).await.unwrap();
    println!(
        "My Drive: {} items in {} ms",
        items.len(),
        started.elapsed().as_millis()
    );

    match items
        .iter()
        // A link file has `#` in its id, and a read of one sends Google nothing.
        .find(|item| {
            item.attributes.kind == Kind::File
                && item.attributes.size > 0
                && !item.id.0.contains('#')
        }) {
        None => println!("no file with bytes at the top of My Drive: reads not checked"),
        Some(file) => {
            let length = file.attributes.size.min(65536) as u32;
            let started = Instant::now();
            let bytes = provider.read(&file.id, 0, length).await.unwrap();
            println!(
                "read of {} bytes in {} ms",
                bytes.len(),
                started.elapsed().as_millis()
            );
            assert_eq!(bytes.len(), length as usize);
            let half = bytes.len() / 2;
            let second = provider
                .read(&file.id, half as u64, (bytes.len() - half) as u32)
                .await
                .unwrap();
            assert!(second == bytes[half..], "a read at an offset differs");
            let at_end = provider
                .read(&file.id, file.attributes.size, 64)
                .await
                .unwrap();
            assert!(at_end.is_empty());
            let past = provider
                .read(&file.id, file.attributes.size + 10, 64)
                .await
                .unwrap();
            assert!(past.is_empty());
        }
    }

    match items
        .iter()
        .find(|item| item.attributes.kind == Kind::File && item.attributes.size == 0)
    {
        None => println!("no empty file at the top of My Drive: its read not checked"),
        Some(file) => {
            let bytes = provider.read(&file.id, 0, 64).await.unwrap();
            assert!(bytes.is_empty());
        }
    }

    let shared = top
        .iter()
        .find(|item| item.name == "Shared drives")
        .unwrap();
    let drives = provider.list(&shared.id).await.unwrap();
    println!("{} shared drives", drives.len());
    match drives.first() {
        None => println!("no shared drive: shared drives not checked"),
        Some(drive) => {
            let found = provider.item(&drive.id).await.unwrap();
            assert!(
                found == *drive,
                "a shared drive's top folder differs from its line in the listing"
            );
            let started = Instant::now();
            let children = provider.list(&drive.id).await.unwrap();
            println!(
                "first shared drive: {} items in {} ms",
                children.len(),
                started.elapsed().as_millis()
            );
            if let Some(child) = children.first() {
                provider.item(&child.id).await.unwrap();
            }
        }
    }
}
