//! Manual check of the three change calls on one real Google account.
//!
//! It CHANGES the account named by `MARLIN_GDRIVE_LIVE_ACCOUNT`. It makes one folder
//! `Marlin live test {unix seconds}` at the top of My Drive and works only inside it.
//! One step moves a folder named `Marlin live test {unix seconds} excursion` to the top
//! of My Drive and back. It never touches a shared drive. It trashes the test folder at the end.
//!
//! It prints the `{unix seconds}` first. If it fails midway, trash the two folders whose
//! names hold that number by hand: the test folder, and the excursion folder if it is at
//! the top of My Drive.
//!
//! It prints counts, statuses, and times only, and never an ID, also when an assertion fails.
//!
//! Run:
//! `MARLIN_GDRIVE_LIVE_ACCOUNT=<email> cargo test -p marlin-gdrive --test live_changes -- --ignored --nocapture`

use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use marlin_gdrive::{AccessTokens, Config, DriveProvider, OAuthClient, SavedSignIn};
use marlin_vfs::{ItemId, Provider, VfsError};

#[tokio::test]
#[ignore = "changes a real Google account and reads the real keychain: see the header"]
async fn a_real_account_makes_a_folder_and_changes_it() {
    let email = std::env::var("MARLIN_GDRIVE_LIVE_ACCOUNT")
        .expect("Set MARLIN_GDRIVE_LIVE_ACCOUNT to the account's email");
    // The loader's own text can hold a file path, so it stays out of the output.
    let client = OAuthClient::load().unwrap_or_else(|_| panic!("no Google OAuth client"));
    let sign_in = Arc::new(SavedSignIn::new(&email, client));
    let tokens: Arc<dyn AccessTokens> = sign_in.clone();
    let provider = DriveProvider::new(tokens, Config::default());
    let api = Config::default().api;
    let http = reqwest::Client::new();
    let root = ItemId("root".to_string());
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    println!("test folders hold {stamp}");

    let started = Instant::now();
    let scratch = provider
        .create_folder(&root, &format!("Marlin live test {stamp}"))
        .await
        .unwrap();
    println!(
        "scratch folder made in {} ms",
        started.elapsed().as_millis()
    );

    let mut made = Vec::new();
    let mut saw = 0;
    let started = Instant::now();
    for n in 1..=5 {
        let folder = provider
            .create_folder(&scratch.id, &format!("folder {n}"))
            .await
            .unwrap();
        let listed = provider.list(&scratch.id).await.unwrap();
        if listed.iter().any(|item| item.id == folder.id) {
            saw += 1;
        }
        made.push(folder);
    }
    println!(
        "{saw} of 5 listings held the new folder, in {} ms",
        started.elapsed().as_millis()
    );

    let started = Instant::now();
    let renamed = provider
        .rename(
            &made[0].id,
            &scratch.id,
            &scratch.id,
            "folder renamed",
            None,
        )
        .await
        .unwrap();
    let listed = provider.list(&scratch.id).await.unwrap();
    let showed = listed
        .iter()
        .any(|item| item.id == renamed.id && item.name == "folder renamed");
    println!(
        "rename shown: {showed} in {} ms",
        started.elapsed().as_millis()
    );

    let started = Instant::now();
    provider.remove(&made[1].id, &scratch.id).await.unwrap();
    let listed = provider.list(&scratch.id).await.unwrap();
    let showed = listed.iter().all(|item| item.id != made[1].id);
    println!(
        "remove shown: {showed} in {} ms",
        started.elapsed().as_millis()
    );

    let started = Instant::now();
    let moved = provider
        .rename(&made[2].id, &scratch.id, &made[3].id, "folder 3", None)
        .await
        .unwrap();
    let parent = provider.list(&scratch.id).await.unwrap();
    let child = provider.list(&made[3].id).await.unwrap();
    let showed = parent.iter().all(|item| item.id != made[2].id)
        && child.iter().any(|item| item.id == moved.id);
    println!(
        "move shown: {showed} in {} ms",
        started.elapsed().as_millis()
    );

    let excursion_name = format!("Marlin live test {stamp} excursion");
    let started = Instant::now();
    let excursion = provider
        .create_folder(&scratch.id, &excursion_name)
        .await
        .unwrap();
    let at_top = provider
        .rename(&excursion.id, &scratch.id, &root, &excursion_name, None)
        .await
        .unwrap();
    provider
        .rename(&at_top.id, &root, &scratch.id, &excursion_name, None)
        .await
        .unwrap();
    println!(
        "excursion moved to My Drive and back in {} ms",
        started.elapsed().as_millis()
    );

    let outer = provider.create_folder(&scratch.id, "outer").await.unwrap();
    let inner = provider.create_folder(&outer.id, "inner").await.unwrap();
    assert!(
        matches!(
            provider
                .rename(&outer.id, &scratch.id, &inner.id, "outer", None)
                .await,
            Err(VfsError::NotSupported)
        ),
        "a folder moved into its own subfolder was not refused"
    );
    println!("folder into its own subfolder: not supported");

    let a = provider.create_folder(&scratch.id, "a").await.unwrap();
    let b = provider.create_folder(&scratch.id, "b").await.unwrap();
    provider
        .rename(&a.id, &scratch.id, &scratch.id, "b", Some(&b.id))
        .await
        .unwrap();
    let listed = provider.list(&scratch.id).await.unwrap();
    let named_b: Vec<_> = listed.iter().filter(|item| item.name == "b").collect();
    assert_eq!(named_b.len(), 1);
    assert!(
        named_b[0].id == a.id,
        "the folder named b is not the moved folder"
    );
    println!("replace left one folder");

    assert!(
        matches!(
            provider.remove(&outer.id, &scratch.id).await,
            Err(VfsError::NotEmpty)
        ),
        "a folder with a child was not refused"
    );
    println!("folder with a child: not empty");

    let token = sign_in.access_token().await.unwrap();
    let status = http
        .post(format!("{api}/files"))
        .bearer_auth(&token)
        .query(&[("supportsAllDrives", "true")])
        .json(&serde_json::json!({
            "name": "notes",
            "mimeType": "application/vnd.google-apps.document",
            "parents": [scratch.id.0],
        }))
        .send()
        .await
        .unwrap()
        .status();
    println!("document create: {}", status.as_u16());
    assert!(status.is_success());
    let listed = provider.list(&scratch.id).await.unwrap();
    let doc = listed
        .iter()
        .find(|item| item.name.ends_with(".document.html"))
        .unwrap();
    let renamed = provider
        .rename(
            &doc.id,
            &scratch.id,
            &scratch.id,
            "renamed.document.html",
            None,
        )
        .await
        .unwrap();
    assert!(renamed.name.ends_with(".document.html"));
    assert!(
        matches!(
            provider
                .rename(&doc.id, &scratch.id, &scratch.id, "renamed.txt", None)
                .await,
            Err(VfsError::NotSupported)
        ),
        "a link file renamed to another ending was not refused"
    );
    provider.remove(&doc.id, &scratch.id).await.unwrap();
    println!("link file renamed, a new ending refused, and the file trashed");

    let token = sign_in.access_token().await.unwrap();
    let generated: serde_json::Value = http
        .get(format!("{api}/files/generateIds"))
        .bearer_auth(&token)
        .query(&[("count", "1"), ("space", "drive"), ("type", "files")])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let new_id = generated["ids"][0].as_str().unwrap();
    let mut statuses = Vec::new();
    for _ in 0..2 {
        let status = http
            .post(format!("{api}/files"))
            .bearer_auth(&token)
            .query(&[("supportsAllDrives", "true")])
            .json(&serde_json::json!({
                "id": new_id,
                "name": "again",
                "mimeType": "application/vnd.google-apps.folder",
                "parents": [scratch.id.0],
            }))
            .send()
            .await
            .unwrap()
            .status();
        statuses.push(status.as_u16());
    }
    println!("create with a new id: {}", statuses[0]);
    println!("create with the same id: {}", statuses[1]);

    let token = sign_in.access_token().await.unwrap();
    let status = http
        .patch(format!("{api}/files/{}", scratch.id.0))
        .bearer_auth(&token)
        .query(&[("supportsAllDrives", "true")])
        .json(&serde_json::json!({ "trashed": true }))
        .send()
        .await
        // The error names the address, which holds the folder's id.
        .unwrap_or_else(|_| panic!("the request to trash the test folder failed"))
        .status();
    println!("scratch folder trashed: {}", status.as_u16());
    assert!(status.is_success());
    // Checked after the cleanup, so a surprise here leaves nothing behind.
    assert_eq!(statuses, [200, 409]);
}
