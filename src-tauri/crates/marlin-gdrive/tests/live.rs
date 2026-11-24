//! Manual checks of one real Google account through [`DriveProvider`].
//!
//! They read the account named by `MARLIN_GDRIVE_LIVE_ACCOUNT` from the OS keychain, so macOS may
//! ask to allow it. They never change anything in the account: they only list, read, and locate.
//!
//! They print counts, sizes, times, and the type words of link files, and never a name, an ID, or file bytes. The assertions on
//! the account's own data print nothing from it either, so the output is safe to paste.
//!
//! Run:
//! `MARLIN_GDRIVE_LIVE_ACCOUNT=<email> cargo test -p marlin-gdrive --test live -- --ignored --nocapture --test-threads=1`

use std::sync::Arc;
use std::time::Instant;

use marlin_gdrive::{Config, DriveProvider, OAuthClient, SavedSignIn};
use marlin_vfs::{ItemId, Kind, Provider};

fn provider() -> DriveProvider {
    let email = std::env::var("MARLIN_GDRIVE_LIVE_ACCOUNT")
        .expect("Set MARLIN_GDRIVE_LIVE_ACCOUNT to the account's email");
    // The loader's own text can hold a file path, so it stays out of the output.
    let client = OAuthClient::load().unwrap_or_else(|_| panic!("no Google OAuth client"));
    DriveProvider::new(
        Arc::new(SavedSignIn::new(&email, client)),
        Config::default(),
    )
}

#[tokio::test]
#[ignore = "reads a real Google account and the real keychain: see the header"]
async fn a_real_account_lists_and_reads() {
    let provider = provider();

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

    // A link file has `#` in its id. A read of one sends Google nothing, and its name ends with the type and `.html`.
    let (links, plain): (Vec<_>, Vec<_>) = items.iter().partition(|item| item.id.0.contains('#'));
    assert!(links.iter().all(|link| link.attributes.read_only));
    // A folder is never read-only, so this counts files the account may only view.
    println!(
        "read-only among the other items at the top of My Drive: {} of {}",
        plain
            .iter()
            .filter(|item| item.attributes.read_only)
            .count(),
        plain.len()
    );

    match plain
        .iter()
        .find(|item| item.attributes.kind == Kind::File && item.attributes.size > 0)
    {
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

    match plain
        .iter()
        .find(|item| item.attributes.kind == Kind::File && item.attributes.size == 0)
    {
        None => println!("no empty file at the top of My Drive: its read not checked"),
        Some(file) => {
            let bytes = provider.read(&file.id, 0, 64).await.unwrap();
            assert!(bytes.is_empty());
        }
    }

    let mut types: Vec<_> = links
        .iter()
        .filter_map(|item| item.name.rsplit('.').nth(1))
        .collect();
    types.sort_unstable();
    types.dedup();
    println!(
        "{} link files at the top of My Drive, of types {types:?}",
        links.len()
    );
    let shortcut = links
        .iter()
        .find(|item| item.name.ends_with(".shortcut.html"));
    match shortcut {
        None => println!("no shortcut at the top of My Drive: its target not checked"),
        Some(shortcut) => {
            // At the top of My Drive an id has no drive part, so the two halves are the shortcut and what it opens.
            let (own, opens) = shortcut.id.0.split_once('#').unwrap();
            assert!(
                own != opens,
                "a shortcut opens itself: Google gave no target"
            );
        }
    }
    let editor = links
        .iter()
        .find(|item| !item.name.ends_with(".shortcut.html"));
    if editor.is_none() {
        println!("no Google editor file at the top of My Drive: its item not checked");
    }
    // `item` asks Google again, so the type and a shortcut's target must come back as they did in the listing.
    for link in editor.into_iter().chain(shortcut) {
        let found = provider.item(&link.id).await.unwrap();
        assert!(
            found == **link,
            "a link file differs from its line in the listing"
        );
        println!(
            "item of a {} link file equals its line in the listing",
            link.name.rsplit('.').nth(1).unwrap()
        );
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

/// `locate` of an item's Drive ID must walk Google's parents back to the place a listing shows it at: one folder and one item in it in My Drive, a link file at the top of My Drive, and a shared drive's top folder and one item in it.
#[tokio::test]
#[ignore = "reads a real Google account and the real keychain: see the header"]
async fn a_real_account_locates_items_where_the_listing_shows_them() {
    let provider = provider();
    let my_drive = ItemId("root".to_string());
    let shared_drives = ItemId("/shared-drives".to_string());
    let mut places: Vec<Vec<ItemId>> = Vec::new();

    let top = provider.list(&my_drive).await.unwrap();
    let first = top.first().expect("My Drive is empty: nothing to locate");
    places.push(vec![my_drive.clone(), first.id.clone()]);
    match top.iter().find(|item| item.attributes.kind == Kind::Folder) {
        None => println!("no folder at the top of My Drive: places in one not checked"),
        Some(folder) => {
            places.push(vec![my_drive.clone(), folder.id.clone()]);
            match provider.list(&folder.id).await.unwrap().first() {
                None => println!(
                    "the first folder of My Drive is empty: an item two levels down not checked"
                ),
                Some(item) => {
                    places.push(vec![my_drive.clone(), folder.id.clone(), item.id.clone()])
                }
            }
        }
    }
    match top.iter().find(|item| item.id.0.contains('#')) {
        None => println!("no link file at the top of My Drive: its place not checked"),
        Some(link) => places.push(vec![my_drive.clone(), link.id.clone()]),
    }
    match provider.list(&shared_drives).await.unwrap().first() {
        None => println!("no shared drive: places in one not checked"),
        Some(drive) => {
            places.push(vec![shared_drives.clone(), drive.id.clone()]);
            match provider.list(&drive.id).await.unwrap().first() {
                None => println!("the first shared drive is empty: an item in it not checked"),
                Some(item) => places.push(vec![
                    shared_drives.clone(),
                    drive.id.clone(),
                    item.id.clone(),
                ]),
            }
        }
    }

    for place in places {
        let reference = provider.reference(place.last().unwrap()).unwrap();
        let started = Instant::now();
        let found = provider.locate(&reference).await.unwrap();
        // No ID in the message: the output must stay safe to paste.
        assert!(
            found.as_ref() == Some(&place),
            "locate of an item {} deep gave another place than the listing",
            place.len()
        );
        println!(
            "locate of an item {} deep in {} ms",
            place.len(),
            started.elapsed().as_millis()
        );
    }
}
