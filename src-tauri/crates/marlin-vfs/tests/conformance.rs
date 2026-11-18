use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use marlin_vfs::{
    Access, Attributes, IfExists, Item, ItemId, Kind, MemoryProvider, Options, Provider, Space,
    Vfs, VfsError, POSIX,
};

fn mount(options: Options) -> (Arc<MemoryProvider>, Arc<Vfs>) {
    let provider = Arc::new(MemoryProvider::new());
    let vfs = Arc::new(Vfs::new(provider.clone(), POSIX, options));
    (provider, vfs)
}

async fn until_in_flight(provider: &MemoryProvider, count: usize) {
    for _ in 0..100 {
        if provider.in_flight() >= count {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("provider calls in flight: {}", provider.in_flight());
}

#[tokio::test]
async fn lookup_returns_kind_size_and_name() {
    let (provider, vfs) = mount(Options::default());
    let docs = provider.add_folder(&provider.root(), "docs");
    provider.add_file(&docs, "a.txt", b"hello");
    let folder = vfs.lookup(Vfs::ROOT, "docs").await.unwrap();
    assert_eq!(folder.name, "docs");
    assert_eq!(folder.attributes.kind, Kind::Folder);
    assert_eq!(folder.attributes.size, 0);
    let file = vfs.lookup(folder.node, "a.txt").await.unwrap();
    assert_eq!(file.name, "a.txt");
    assert_eq!(file.attributes.kind, Kind::File);
    assert_eq!(file.attributes.size, 5);
    assert_eq!(vfs.attributes(file.node).await.unwrap(), file.attributes);
    assert_eq!(
        vfs.attributes(folder.node).await.unwrap(),
        folder.attributes
    );
}

#[tokio::test]
async fn missing_name_and_lookup_inside_a_file() {
    let (provider, vfs) = mount(Options::default());
    provider.add_file(&provider.root(), "a.txt", b"hello");
    assert_eq!(
        vfs.lookup(Vfs::ROOT, "missing").await,
        Err(VfsError::NotFound)
    );
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    assert_eq!(
        vfs.lookup(file.node, "inside").await,
        Err(VfsError::NotAFolder)
    );
}

#[tokio::test]
async fn read_folder_pages_a_stable_listing() {
    let (provider, vfs) = mount(Options::default());
    for index in 0..5 {
        provider.add_file(&provider.root(), &format!("f{index}.txt"), b"x");
    }
    let handle = vfs.open_folder(Vfs::ROOT).await.unwrap();
    let all = vfs.read_folder(handle, 0, 10).unwrap();
    assert_eq!(all.len(), 5);
    let mut paged = Vec::new();
    let mut offset = 0;
    loop {
        let page = vfs.read_folder(handle, offset, 2).unwrap();
        if page.is_empty() {
            break;
        }
        offset += page.len();
        paged.extend(page);
    }
    assert_eq!(paged, all);
    assert!(vfs.read_folder(handle, 5, 2).unwrap().is_empty());
    provider.add_file(&provider.root(), "later.txt", b"y");
    assert_eq!(vfs.read_folder(handle, 0, 10).unwrap(), all);
}

#[tokio::test]
async fn read_ranges() {
    let (provider, vfs) = mount(Options::default());
    provider.add_file(&provider.root(), "a.txt", b"abcdefghij");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Read).await.unwrap();
    assert_eq!(vfs.read(handle, 2, 4).await.unwrap(), b"cdef");
    assert_eq!(vfs.read(handle, 8, 4).await.unwrap(), b"ij");
    assert_eq!(vfs.read(handle, 10, 4).await.unwrap(), b"");
}

#[tokio::test]
async fn released_and_wrong_kind_handles_are_stale() {
    let (provider, vfs) = mount(Options::default());
    provider.add_file(&provider.root(), "a.txt", b"hello");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Read).await.unwrap();
    vfs.release(handle);
    assert_eq!(vfs.read(handle, 0, 1).await, Err(VfsError::StaleHandle));
    let again = vfs.open(file.node, Access::Read).await.unwrap();
    assert_eq!(vfs.read_folder(again, 0, 1), Err(VfsError::StaleHandle));
}

#[tokio::test]
async fn open_checks_kind_and_write_access() {
    let (provider, vfs) = mount(Options::default());
    let docs = provider.add_folder(&provider.root(), "docs");
    provider.add_file(&docs, "a.txt", b"hello");
    let folder = vfs.lookup(Vfs::ROOT, "docs").await.unwrap();
    assert_eq!(
        vfs.open(folder.node, Access::Read).await,
        Err(VfsError::IsAFolder)
    );
    let file = vfs.lookup(folder.node, "a.txt").await.unwrap();
    assert_eq!(vfs.open_folder(file.node).await, Err(VfsError::NotAFolder));
    assert_eq!(
        vfs.open(file.node, Access::Write).await,
        Err(VfsError::ReadOnly)
    );
}

#[tokio::test]
async fn space_matches_the_provider() {
    let (provider, vfs) = mount(Options::default());
    assert_eq!(vfs.space().await.unwrap(), provider.space().await.unwrap());
}

#[tokio::test]
async fn rename_keeps_the_node_and_the_open_handle() {
    let (provider, vfs) = mount(Options::default());
    let root = provider.root();
    let folder_a = provider.add_folder(&root, "a");
    let folder_b = provider.add_folder(&root, "b");
    let file = provider.add_file(&folder_a, "note.txt", b"body");
    let folder = vfs.lookup(Vfs::ROOT, "a").await.unwrap();
    let note = vfs.lookup(folder.node, "note.txt").await.unwrap();
    let handle = vfs.open(note.node, Access::Read).await.unwrap();
    provider.remote_rename(&file, &folder_b, "moved.txt");
    vfs.refresh();
    let destination = vfs.lookup(Vfs::ROOT, "b").await.unwrap();
    let moved = vfs.lookup(destination.node, "moved.txt").await.unwrap();
    assert_eq!(moved.node, note.node);
    assert_eq!(vfs.read(handle, 0, 4).await.unwrap(), b"body");
    assert_eq!(
        vfs.lookup(folder.node, "note.txt").await,
        Err(VfsError::NotFound)
    );
}

#[tokio::test]
async fn forget_drops_a_node_unless_a_handle_holds_it() {
    let (provider, vfs) = mount(Options::default());
    provider.add_file(&provider.root(), "a.txt", b"hello");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    vfs.forget(file.node, 1);
    assert_eq!(vfs.attributes(file.node).await, Err(VfsError::StaleHandle));
    let again = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    assert!(again.node > file.node);
    let handle = vfs.open(again.node, Access::Read).await.unwrap();
    vfs.forget(again.node, 1_000_000);
    assert!(vfs.attributes(again.node).await.is_ok());
    vfs.release(handle);
    assert_eq!(vfs.attributes(again.node).await, Err(VfsError::StaleHandle));
}

#[tokio::test(start_paused = true)]
async fn open_is_stale_when_the_node_is_forgotten_during_the_call() {
    let (provider, vfs) = mount(Options::default());
    provider.add_file(&provider.root(), "a.txt", b"hello");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    vfs.refresh();
    provider.set_delay(Duration::from_secs(5));
    let vfs_for_open = Arc::clone(&vfs);
    let node = file.node;
    let task = tokio::spawn(async move { vfs_for_open.open(node, Access::Read).await });
    until_in_flight(&provider, 1).await;
    vfs.forget(node, u64::MAX);
    assert_eq!(task.await.unwrap(), Err(VfsError::StaleHandle));
}

#[tokio::test(start_paused = true)]
async fn refresh_during_a_listing_discards_the_old_listing() {
    let (provider, vfs) = mount(Options::default());
    provider.add_file(&provider.root(), "old.txt", b"old");
    provider.set_delay(Duration::from_secs(5));
    let vfs_for_lookup = Arc::clone(&vfs);
    let task = tokio::spawn(async move { vfs_for_lookup.lookup(Vfs::ROOT, "old.txt").await });
    until_in_flight(&provider, 1).await;
    provider.add_file(&provider.root(), "new.txt", b"new");
    vfs.refresh();
    assert_eq!(task.await.unwrap().unwrap().name, "old.txt");
    provider.set_delay(Duration::ZERO);
    assert_eq!(
        vfs.lookup(Vfs::ROOT, "new.txt").await.unwrap().name,
        "new.txt"
    );
}

#[tokio::test]
async fn duplicate_names_and_a_slash_map_to_distinct_files() {
    let (provider, vfs) = mount(Options::default());
    let root = provider.root();
    provider.add_file(&root, "same.txt", b"first");
    provider.add_file(&root, "same.txt", b"second");
    provider.add_file(&root, "a/b", b"slash");
    let handle = vfs.open_folder(Vfs::ROOT).await.unwrap();
    let entries = vfs.read_folder(handle, 0, 10).unwrap();
    let names: HashSet<_> = entries.iter().map(|entry| entry.name.clone()).collect();
    assert_eq!(names.len(), 3);
    for (name, contents) in [
        ("same.txt", &b"first"[..]),
        ("same (2).txt", &b"second"[..]),
        ("a%2Fb", &b"slash"[..]),
    ] {
        let entry = vfs.lookup(Vfs::ROOT, name).await.unwrap();
        let file = vfs.open(entry.node, Access::Read).await.unwrap();
        assert_eq!(vfs.read(file, 0, 8).await.unwrap(), contents, "{name}");
    }
}

#[tokio::test]
async fn provider_errors_pass_through() {
    let (provider, vfs) = mount(Options::default());
    provider.add_file(&provider.root(), "a.txt", b"hello");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    vfs.refresh();
    for error in [VfsError::Offline, VfsError::SignInNeeded] {
        provider.fail_with(Some(error.clone()));
        assert_eq!(vfs.lookup(Vfs::ROOT, "a.txt").await, Err(error.clone()));
        assert_eq!(vfs.attributes(file.node).await, Err(error.clone()));
        assert_eq!(vfs.open(file.node, Access::Read).await, Err(error.clone()));
        assert_eq!(vfs.space().await, Err(error.clone()));
    }
}

#[tokio::test(start_paused = true)]
async fn read_times_out_and_the_next_read_works() {
    let timeout = Duration::from_secs(1);
    let (provider, vfs) = mount(Options {
        call_timeout: timeout,
        ..Options::default()
    });
    provider.add_file(&provider.root(), "a.txt", b"hello");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Read).await.unwrap();
    provider.set_delay(Duration::from_secs(60));
    assert_eq!(vfs.read(handle, 0, 5).await, Err(VfsError::TimedOut));
    provider.set_delay(Duration::ZERO);
    assert_eq!(vfs.read(handle, 0, 5).await.unwrap(), b"hello");
}

#[tokio::test(start_paused = true)]
async fn reads_stay_within_the_call_limit() {
    let (provider, vfs) = mount(Options {
        max_calls: 2,
        ..Options::default()
    });
    provider.add_file(&provider.root(), "a.txt", b"hello");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Read).await.unwrap();
    provider.set_delay(Duration::from_secs(2));
    let mut reads = Vec::new();
    for _ in 0..8 {
        let vfs = Arc::clone(&vfs);
        reads.push(tokio::spawn(async move { vfs.read(handle, 0, 5).await }));
    }
    for read in reads {
        assert_eq!(read.await.unwrap().unwrap(), b"hello");
    }
    assert_eq!(provider.peak_calls(), 2);
}

#[tokio::test(start_paused = true)]
async fn aborted_read_returns_its_permit() {
    let (provider, vfs) = mount(Options {
        max_calls: 1,
        ..Options::default()
    });
    provider.add_file(&provider.root(), "a.txt", b"hello");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Read).await.unwrap();
    provider.set_delay(Duration::from_secs(60));
    let vfs_for_read = Arc::clone(&vfs);
    let task = tokio::spawn(async move { vfs_for_read.read(handle, 0, 5).await });
    until_in_flight(&provider, 1).await;
    task.abort();
    let _ = task.await;
    provider.set_delay(Duration::ZERO);
    assert_eq!(vfs.read(handle, 0, 5).await.unwrap(), b"hello");
}

#[tokio::test(start_paused = true)]
async fn concurrent_lookups_share_one_node() {
    let (provider, vfs) = mount(Options::default());
    provider.add_file(&provider.root(), "a.txt", b"hello");
    provider.set_delay(Duration::from_secs(1));
    let mut tasks = Vec::new();
    for _ in 0..16 {
        let vfs = Arc::clone(&vfs);
        tasks.push(tokio::spawn(async move {
            vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap().node
        }));
    }
    let mut nodes = Vec::new();
    for task in tasks {
        nodes.push(task.await.unwrap());
    }
    assert!(nodes.iter().all(|node| *node == nodes[0]));
}

#[tokio::test(start_paused = true)]
async fn new_file_shows_up_after_the_listing_expires_or_a_refresh() {
    let ttl = Duration::from_secs(10);
    let (provider, vfs) = mount(Options {
        listing_ttl: ttl,
        ..Options::default()
    });
    provider.add_file(&provider.root(), "a.txt", b"a");
    vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    provider.add_file(&provider.root(), "b.txt", b"b");
    assert_eq!(
        vfs.lookup(Vfs::ROOT, "b.txt").await,
        Err(VfsError::NotFound)
    );
    tokio::time::advance(ttl - Duration::from_millis(1)).await;
    assert_eq!(
        vfs.lookup(Vfs::ROOT, "b.txt").await,
        Err(VfsError::NotFound)
    );
    tokio::time::advance(Duration::from_millis(1)).await;
    assert_eq!(vfs.lookup(Vfs::ROOT, "b.txt").await.unwrap().name, "b.txt");
    provider.add_file(&provider.root(), "c.txt", b"c");
    assert_eq!(
        vfs.lookup(Vfs::ROOT, "c.txt").await,
        Err(VfsError::NotFound)
    );
    vfs.refresh();
    assert_eq!(vfs.lookup(Vfs::ROOT, "c.txt").await.unwrap().name, "c.txt");
}

#[tokio::test(start_paused = true)]
async fn a_full_cache_drops_its_oldest_listing() {
    let (provider, vfs) = mount(Options {
        max_listings: 2,
        ..Options::default()
    });
    let root = provider.root();
    let older = provider.add_folder(&root, "older");
    let third = provider.add_folder(&root, "third");
    provider.add_file(&third, "c.txt", b"c");
    let older_node = vfs.lookup(Vfs::ROOT, "older").await.unwrap().node;
    let third_node = vfs.lookup(Vfs::ROOT, "third").await.unwrap().node;
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(
        vfs.lookup(older_node, "late.txt").await,
        Err(VfsError::NotFound)
    );
    provider.add_file(&root, "late.txt", b"late");
    provider.add_file(&older, "late.txt", b"late");
    tokio::time::advance(Duration::from_secs(1)).await;
    vfs.lookup(third_node, "c.txt").await.unwrap();
    assert_eq!(
        vfs.lookup(older_node, "late.txt").await,
        Err(VfsError::NotFound)
    );
    assert!(vfs.lookup(Vfs::ROOT, "late.txt").await.is_ok());
}

#[tokio::test(start_paused = true)]
async fn a_watcher_hears_how_every_provider_call_ended() {
    let provider = Arc::new(MemoryProvider::new());
    let heard = Arc::new(Mutex::new(Vec::new()));
    let heard_for_watch = Arc::clone(&heard);
    let options = Options {
        space_ttl: Duration::ZERO,
        ..Options::default()
    };
    let vfs = Vfs::new(provider.clone(), POSIX, options).watch_calls(move |error| {
        heard_for_watch.lock().unwrap().push(error.cloned());
    });

    vfs.space().await.unwrap();
    assert_eq!(*heard.lock().unwrap(), vec![None]);

    provider.fail_with(Some(VfsError::Offline));
    assert_eq!(vfs.space().await, Err(VfsError::Offline));
    provider.fail_with(None);
    assert_eq!(*heard.lock().unwrap(), vec![None, Some(VfsError::Offline)]);

    provider.set_delay(options.call_timeout + Duration::from_secs(1));
    assert_eq!(vfs.space().await, Err(VfsError::TimedOut));
    provider.set_delay(Duration::ZERO);
    assert_eq!(
        *heard.lock().unwrap(),
        vec![None, Some(VfsError::Offline), Some(VfsError::TimedOut)]
    );

    assert_eq!(
        vfs.lookup(Vfs::ROOT, "missing").await,
        Err(VfsError::NotFound)
    );
    assert_eq!(
        vfs.lookup(Vfs::ROOT, "missing").await,
        Err(VfsError::NotFound)
    );
    assert_eq!(
        *heard.lock().unwrap(),
        vec![
            None,
            Some(VfsError::Offline),
            Some(VfsError::TimedOut),
            None
        ]
    );

    provider.add_file(&provider.root(), "a.txt", b"hello");
    vfs.refresh();
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    assert_eq!(
        *heard.lock().unwrap(),
        vec![
            None,
            Some(VfsError::Offline),
            Some(VfsError::TimedOut),
            None,
            None,
        ]
    );
    vfs.attributes(file.node).await.unwrap();
    assert_eq!(heard.lock().unwrap().len(), 5);

    provider.set_delay(Duration::from_secs(5));
    let cancelled = tokio::time::timeout(Duration::from_secs(1), vfs.space()).await;
    assert!(cancelled.is_err());
    assert_eq!(heard.lock().unwrap().len(), 5);
}

#[tokio::test]
async fn an_open_folder_holds_its_own_node() {
    let (provider, vfs) = mount(Options::default());
    provider.add_folder(&provider.root(), "docs");
    let folder = vfs.lookup(Vfs::ROOT, "docs").await.unwrap();
    let handle = vfs.open_folder(folder.node).await.unwrap();
    vfs.forget(folder.node, 1);
    assert!(vfs.attributes(folder.node).await.is_ok());
    vfs.release(handle);
    assert_eq!(
        vfs.attributes(folder.node).await,
        Err(VfsError::StaleHandle)
    );
}

#[tokio::test]
async fn zero_listings_turns_the_cache_off() {
    let (provider, vfs) = mount(Options {
        max_listings: 0,
        ..Options::default()
    });
    provider.add_file(&provider.root(), "a.txt", b"a");
    vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    provider.add_file(&provider.root(), "b.txt", b"b");
    assert_eq!(vfs.lookup(Vfs::ROOT, "b.txt").await.unwrap().name, "b.txt");
}

#[tokio::test]
async fn attributes_after_a_listing_ask_the_provider_nothing() {
    let (provider, vfs) = mount(Options::default());
    for name in ["a.txt", "b.txt", "c.txt"] {
        provider.add_file(&provider.root(), name, b"hello");
    }
    let handle = vfs.open_folder(Vfs::ROOT).await.unwrap();
    let entries = vfs.read_folder(handle, 0, 10).unwrap();
    assert_eq!(entries.len(), 3);
    for entry in &entries {
        assert_eq!(vfs.attributes(entry.node).await.unwrap(), entry.attributes);
    }
    assert_eq!(provider.calls(), 1);
    let found = vfs.lookup(Vfs::ROOT, &entries[0].name).await.unwrap();
    vfs.open(found.node, Access::Read).await.unwrap();
    assert_eq!(provider.calls(), 1);
}

#[tokio::test(start_paused = true)]
async fn attributes_are_asked_again_when_the_answer_is_too_old() {
    let ttl = Duration::from_secs(10);
    let (provider, vfs) = mount(Options {
        attributes_ttl: ttl,
        listing_ttl: ttl,
        ..Options::default()
    });
    let id = provider.add_file(&provider.root(), "a.txt", b"hello");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let calls = provider.calls();
    provider.write(&id, b"helloworld");
    tokio::time::advance(ttl - Duration::from_millis(1)).await;
    assert_eq!(vfs.attributes(file.node).await.unwrap().size, 5);
    assert_eq!(provider.calls(), calls);
    tokio::time::advance(Duration::from_millis(1)).await;
    assert_eq!(vfs.attributes(file.node).await.unwrap().size, 10);
    assert_eq!(provider.calls(), calls + 1);
    assert_eq!(vfs.attributes(file.node).await.unwrap().size, 10);
    assert_eq!(provider.calls(), calls + 1);
}

#[tokio::test]
async fn refresh_makes_stored_attributes_stale() {
    let (provider, vfs) = mount(Options::default());
    let id = provider.add_file(&provider.root(), "a.txt", b"hello");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    provider.write(&id, b"helloworld");
    vfs.refresh();
    assert_eq!(vfs.attributes(file.node).await.unwrap().size, 10);
}

#[tokio::test(start_paused = true)]
async fn a_refresh_during_an_attributes_call_keeps_its_answer_out() {
    let (provider, vfs) = mount(Options::default());
    let id = provider.add_file(&provider.root(), "a.txt", b"hello");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    vfs.refresh();
    provider.set_delay(Duration::from_secs(1));
    let vfs_for_attributes = Arc::clone(&vfs);
    let node = file.node;
    let task = tokio::spawn(async move { vfs_for_attributes.attributes(node).await });
    until_in_flight(&provider, 1).await;
    provider.write(&id, b"helloworld");
    vfs.refresh();
    assert_eq!(task.await.unwrap().unwrap().size, 5);
    provider.set_delay(Duration::ZERO);
    assert_eq!(vfs.attributes(file.node).await.unwrap().size, 10);
}

#[tokio::test(start_paused = true)]
async fn a_cancelled_attributes_call_stores_nothing() {
    let (provider, vfs) = mount(Options::default());
    let id = provider.add_file(&provider.root(), "a.txt", b"hello");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    vfs.refresh();
    provider.set_delay(Duration::from_secs(60));
    let vfs_for_attributes = Arc::clone(&vfs);
    let node = file.node;
    let task = tokio::spawn(async move { vfs_for_attributes.attributes(node).await });
    until_in_flight(&provider, 1).await;
    task.abort();
    let _ = task.await;
    provider.write(&id, b"helloworld");
    provider.set_delay(Duration::ZERO);
    let calls = provider.calls();
    assert_eq!(vfs.attributes(file.node).await.unwrap().size, 10);
    assert_eq!(provider.calls(), calls + 1);
}

#[tokio::test]
async fn a_stored_size_does_not_cut_a_read_short() {
    let (provider, vfs) = mount(Options::default());
    let id = provider.add_file(&provider.root(), "a.txt", b"hello");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Read).await.unwrap();
    provider.write(&id, b"helloworld");
    assert_eq!(vfs.attributes(file.node).await.unwrap().size, 5);
    assert_eq!(vfs.read(handle, 5, 5).await.unwrap(), b"world");
}

#[tokio::test(start_paused = true)]
async fn an_older_listing_does_not_replace_a_later_answer() {
    let (provider, vfs) = mount(Options {
        attributes_ttl: Duration::from_secs(2),
        listing_ttl: Duration::from_secs(10),
        ..Options::default()
    });
    let id = provider.add_file(&provider.root(), "a.txt", b"hello");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let calls = provider.calls();
    provider.write(&id, b"helloworld");
    tokio::time::advance(Duration::from_secs(3)).await;
    assert_eq!(vfs.attributes(file.node).await.unwrap().size, 10);
    assert_eq!(provider.calls(), calls + 1);
    let again = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    assert_eq!(again.attributes.size, 10);
    assert_eq!(vfs.attributes(file.node).await.unwrap().size, 10);
    assert_eq!(provider.calls(), calls + 1);
}

#[tokio::test(start_paused = true)]
async fn a_slow_answer_does_not_replace_one_asked_for_later() {
    let (provider, vfs) = mount(Options::default());
    let id = provider.add_file(&provider.root(), "a.txt", b"hello");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    vfs.refresh();
    provider.set_delay(Duration::from_secs(3));
    let vfs_for_attributes = Arc::clone(&vfs);
    let node = file.node;
    let first = tokio::spawn(async move { vfs_for_attributes.attributes(node).await });
    until_in_flight(&provider, 1).await;
    tokio::time::advance(Duration::from_millis(1)).await;
    provider.write(&id, b"helloworld");
    provider.set_delay(Duration::from_secs(1));
    assert_eq!(vfs.attributes(file.node).await.unwrap().size, 10);
    assert_eq!(first.await.unwrap().unwrap().size, 10);
    provider.set_delay(Duration::ZERO);
    let calls = provider.calls();
    assert_eq!(vfs.attributes(file.node).await.unwrap().size, 10);
    assert_eq!(provider.calls(), calls);
}

#[tokio::test]
async fn zero_attributes_ttl_turns_the_cache_off() {
    let (provider, vfs) = mount(Options {
        attributes_ttl: Duration::ZERO,
        ..Options::default()
    });
    let id = provider.add_file(&provider.root(), "a.txt", b"hello");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    provider.write(&id, b"helloworld");
    assert_eq!(vfs.attributes(file.node).await.unwrap().size, 10);
}

#[tokio::test(start_paused = true)]
async fn space_is_asked_again_when_the_answer_is_too_old_or_after_a_refresh() {
    let ttl = Duration::from_secs(30);
    let (provider, vfs) = mount(Options {
        space_ttl: ttl,
        ..Options::default()
    });
    let space = vfs.space().await.unwrap();
    tokio::time::advance(ttl - Duration::from_millis(1)).await;
    assert_eq!(vfs.space().await.unwrap(), space);
    assert_eq!(provider.calls(), 1);
    tokio::time::advance(Duration::from_millis(1)).await;
    vfs.space().await.unwrap();
    assert_eq!(provider.calls(), 2);
    vfs.refresh();
    vfs.space().await.unwrap();
    assert_eq!(provider.calls(), 3);
}

#[tokio::test]
async fn a_failed_space_call_is_not_kept() {
    let (provider, vfs) = mount(Options::default());
    provider.fail_with(Some(VfsError::Offline));
    assert_eq!(vfs.space().await, Err(VfsError::Offline));
    provider.fail_with(None);
    assert!(vfs.space().await.is_ok());
    assert_eq!(provider.calls(), 2);
    assert!(vfs.space().await.is_ok());
    assert_eq!(provider.calls(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_slow_listing_is_reused_from_when_it_came() {
    let options = Options::default();
    let (provider, vfs) = mount(options);
    provider.add_file(&provider.root(), "a.txt", b"hello");
    provider.set_delay(options.listing_ttl + Duration::from_secs(1));
    let handle = vfs.open_folder(Vfs::ROOT).await.unwrap();
    vfs.release(handle);
    provider.set_delay(Duration::ZERO);
    tokio::time::advance(options.listing_ttl - Duration::from_millis(1)).await;
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    vfs.attributes(file.node).await.unwrap();
    assert_eq!(provider.calls(), 1);
    tokio::time::advance(Duration::from_millis(1)).await;
    vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    assert_eq!(provider.calls(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_slow_listing_does_not_replace_one_asked_for_later() {
    let (provider, vfs) = mount(Options::default());
    provider.add_file(&provider.root(), "a.txt", b"a");
    provider.set_delay(Duration::from_secs(3));
    let vfs_for_lookup = Arc::clone(&vfs);
    let first = tokio::spawn(async move { vfs_for_lookup.lookup(Vfs::ROOT, "a.txt").await });
    until_in_flight(&provider, 1).await;
    tokio::time::advance(Duration::from_millis(1)).await;
    provider.add_file(&provider.root(), "b.txt", b"b");
    provider.set_delay(Duration::from_secs(1));
    vfs.lookup(Vfs::ROOT, "b.txt").await.unwrap();
    first.await.unwrap().unwrap();
    provider.set_delay(Duration::ZERO);
    let calls = provider.calls();
    assert_eq!(vfs.lookup(Vfs::ROOT, "b.txt").await.unwrap().name, "b.txt");
    assert_eq!(provider.calls(), calls);
}

#[tokio::test]
async fn a_new_folder_shows_at_once() {
    let (provider, vfs) = mount(Options::default());
    provider.add_file(&provider.root(), "a.txt", b"a");
    vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let calls = provider.calls();
    let created = vfs.create_folder(Vfs::ROOT, "docs").await.unwrap();
    assert_eq!(created.name, "docs");
    assert_eq!(created.attributes.kind, Kind::Folder);
    let found = vfs.lookup(Vfs::ROOT, "docs").await.unwrap();
    assert_eq!(found.node, created.node);
    assert_eq!(provider.calls(), calls + 1);
}

#[tokio::test]
async fn create_folder_on_a_taken_name_conflicts() {
    let (provider, vfs) = mount(Options::default());
    provider.add_file(&provider.root(), "a.txt", b"a");
    vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let calls = provider.calls();
    assert_eq!(
        vfs.create_folder(Vfs::ROOT, "a.txt").await,
        Err(VfsError::Conflict)
    );
    assert_eq!(provider.calls(), calls);
}

#[tokio::test]
async fn a_created_name_reaches_the_provider_unescaped() {
    let (provider, vfs) = mount(Options::default());
    let created = vfs.create_folder(Vfs::ROOT, "a%2Fb").await.unwrap();
    let remote = provider.list(&provider.root()).await.unwrap();
    assert_eq!(remote.len(), 1);
    assert_eq!(remote[0].name, "a/b");
    let found = vfs.lookup(Vfs::ROOT, "a%2Fb").await.unwrap();
    assert_eq!(found.node, created.node);
    assert_eq!(found.name, "a%2Fb");
}

#[tokio::test]
async fn a_remote_name_with_percent_shows_as_it_is() {
    let (provider, vfs) = mount(Options::default());
    provider.add_file(&provider.root(), "My%20File.pdf", b"bytes");
    let file = vfs.lookup(Vfs::ROOT, "My%20File.pdf").await.unwrap();
    assert_eq!(file.name, "My%20File.pdf");
    let handle = vfs.open(file.node, Access::Read).await.unwrap();
    assert_eq!(vfs.read(handle, 0, 8).await.unwrap(), b"bytes");
    assert_eq!(
        vfs.lookup(Vfs::ROOT, "My%2520File.pdf").await,
        Err(VfsError::NotFound)
    );
}

#[tokio::test]
async fn names_with_percent_can_be_made_and_found() {
    let (provider, vfs) = mount(Options::default());
    provider.add_file(&provider.root(), "x.txt", b"x");
    for (shown, remote_name) in [
        ("My%20File.pdf", "My%20File.pdf"),
        ("100%.txt", "100%.txt"),
        ("%41", "%41"),
        ("a%2Fb", "a/b"),
    ] {
        let created = vfs.create_folder(Vfs::ROOT, shown).await.unwrap();
        let remote = provider.list(&provider.root()).await.unwrap();
        assert!(
            remote.iter().any(|item| item.name == remote_name),
            "{remote_name}"
        );
        let found = vfs.lookup(Vfs::ROOT, shown).await.unwrap();
        assert_eq!(found.node, created.node);
    }
    let file = vfs.lookup(Vfs::ROOT, "x.txt").await.unwrap();
    let mut current = "x.txt";
    for (new_name, remote_name) in [
        ("r%20x", "r%20x"),
        ("r100%", "r100%"),
        ("%42", "%42"),
        ("r%2Fx", "r/x"),
    ] {
        vfs.rename(Vfs::ROOT, current, Vfs::ROOT, new_name, IfExists::Replace)
            .await
            .unwrap();
        let found = vfs.lookup(Vfs::ROOT, new_name).await.unwrap();
        assert_eq!(found.node, file.node);
        let remote = provider.list(&provider.root()).await.unwrap();
        assert!(
            remote.iter().any(|item| item.name == remote_name),
            "{remote_name}"
        );
        current = new_name;
    }
}

#[tokio::test]
async fn a_rename_onto_a_name_the_remote_holds_twice_is_refused() {
    let (provider, vfs) = mount(Options::default());
    provider.add_file(&provider.root(), "a.txt", b"first");
    provider.add_file(&provider.root(), "a.txt", b"second");
    provider.add_file(&provider.root(), "b.txt", b"b");
    let plain = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let duplicate = vfs.lookup(Vfs::ROOT, "a (2).txt").await.unwrap();
    let other = vfs.lookup(Vfs::ROOT, "b.txt").await.unwrap();
    let calls = provider.calls();
    assert_eq!(
        vfs.rename(Vfs::ROOT, "b.txt", Vfs::ROOT, "a.txt", IfExists::Replace)
            .await,
        Err(VfsError::Conflict)
    );
    assert_eq!(provider.calls(), calls);
    assert_eq!(
        vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap().node,
        plain.node
    );
    assert_eq!(
        vfs.lookup(Vfs::ROOT, "a (2).txt").await.unwrap().node,
        duplicate.node
    );
    assert_eq!(
        vfs.lookup(Vfs::ROOT, "b.txt").await.unwrap().node,
        other.node
    );
}

#[tokio::test]
async fn the_shown_duplicate_can_take_the_plain_name() {
    let (provider, vfs) = mount(Options::default());
    provider.add_file(&provider.root(), "a.txt", b"first");
    provider.add_file(&provider.root(), "a.txt", b"second");
    let duplicate = vfs.lookup(Vfs::ROOT, "a (2).txt").await.unwrap();
    vfs.rename(
        Vfs::ROOT,
        "a (2).txt",
        Vfs::ROOT,
        "a.txt",
        IfExists::Replace,
    )
    .await
    .unwrap();
    assert_eq!(
        vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap().node,
        duplicate.node
    );
    assert_eq!(
        vfs.lookup(Vfs::ROOT, "a (2).txt").await,
        Err(VfsError::NotFound)
    );
    assert_eq!(provider.list(&provider.root()).await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_rename_onto_a_shown_duplicate_replaces_that_item() {
    let (provider, vfs) = mount(Options::default());
    provider.add_file(&provider.root(), "a.txt", b"first");
    provider.add_file(&provider.root(), "a.txt", b"second");
    provider.add_file(&provider.root(), "b.txt", b"b");
    let plain = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let moved = vfs.lookup(Vfs::ROOT, "b.txt").await.unwrap();
    vfs.rename(
        Vfs::ROOT,
        "b.txt",
        Vfs::ROOT,
        "a (2).txt",
        IfExists::Replace,
    )
    .await
    .unwrap();
    assert_eq!(
        vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap().node,
        plain.node
    );
    assert_eq!(
        vfs.lookup(Vfs::ROOT, "a (2).txt").await.unwrap().node,
        moved.node
    );
    let mut names: Vec<_> = provider
        .list(&provider.root())
        .await
        .unwrap()
        .into_iter()
        .map(|item| item.name)
        .collect();
    names.sort();
    assert_eq!(names, ["a (2).txt", "a.txt"]);
}

#[tokio::test]
async fn a_shown_duplicate_cannot_take_a_name_held_three_times() {
    let (provider, vfs) = mount(Options::default());
    for contents in [&b"first"[..], b"second", b"third"] {
        provider.add_file(&provider.root(), "a.txt", contents);
    }
    let third = vfs.lookup(Vfs::ROOT, "a (3).txt").await.unwrap();
    let calls = provider.calls();
    assert_eq!(
        vfs.rename(
            Vfs::ROOT,
            "a (3).txt",
            Vfs::ROOT,
            "a.txt",
            IfExists::Replace
        )
        .await,
        Err(VfsError::Conflict)
    );
    assert_eq!(provider.calls(), calls);
    assert_eq!(
        vfs.lookup(Vfs::ROOT, "a (3).txt").await.unwrap().node,
        third.node
    );
}

#[tokio::test]
async fn a_moved_duplicate_keeps_its_shown_name() {
    let (provider, vfs) = mount(Options::default());
    let root = provider.root();
    let folder_x = provider.add_folder(&root, "x");
    let folder_y = provider.add_folder(&root, "y");
    provider.add_file(&folder_x, "a.txt", b"first");
    provider.add_file(&folder_x, "a.txt", b"second");
    let x = vfs.lookup(Vfs::ROOT, "x").await.unwrap();
    let y = vfs.lookup(Vfs::ROOT, "y").await.unwrap();
    let duplicate = vfs.lookup(x.node, "a (2).txt").await.unwrap();
    vfs.rename(x.node, "a (2).txt", y.node, "a (2).txt", IfExists::Replace)
        .await
        .unwrap();
    let remote = provider.list(&folder_y).await.unwrap();
    assert_eq!(remote.len(), 1);
    assert_eq!(remote[0].name, "a (2).txt");
    assert_eq!(
        vfs.lookup(y.node, "a (2).txt").await.unwrap().node,
        duplicate.node
    );
    assert!(vfs.lookup(x.node, "a.txt").await.is_ok());
    assert_eq!(
        vfs.lookup(x.node, "a (2).txt").await,
        Err(VfsError::NotFound)
    );
}

#[tokio::test]
async fn remove_drops_a_file_and_an_empty_folder_at_once() {
    let (provider, vfs) = mount(Options::default());
    provider.add_file(&provider.root(), "a.txt", b"a");
    provider.add_folder(&provider.root(), "empty");
    vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    vfs.lookup(Vfs::ROOT, "empty").await.unwrap();
    let calls = provider.calls();
    vfs.remove(Vfs::ROOT, "a.txt", Kind::File).await.unwrap();
    vfs.remove(Vfs::ROOT, "empty", Kind::Folder).await.unwrap();
    assert_eq!(
        vfs.lookup(Vfs::ROOT, "a.txt").await,
        Err(VfsError::NotFound)
    );
    assert_eq!(
        vfs.lookup(Vfs::ROOT, "empty").await,
        Err(VfsError::NotFound)
    );
    assert_eq!(provider.calls(), calls + 2);
}

#[tokio::test]
async fn remove_with_the_wrong_kind_leaves_the_item() {
    let (provider, vfs) = mount(Options::default());
    provider.add_file(&provider.root(), "a.txt", b"a");
    provider.add_folder(&provider.root(), "docs");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let folder = vfs.lookup(Vfs::ROOT, "docs").await.unwrap();
    assert_eq!(
        vfs.remove(Vfs::ROOT, "docs", Kind::File).await,
        Err(VfsError::IsAFolder)
    );
    assert_eq!(
        vfs.remove(Vfs::ROOT, "a.txt", Kind::Folder).await,
        Err(VfsError::NotAFolder)
    );
    assert_eq!(
        vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap().node,
        file.node
    );
    assert_eq!(
        vfs.lookup(Vfs::ROOT, "docs").await.unwrap().node,
        folder.node
    );
}

#[tokio::test]
async fn remove_of_a_folder_that_holds_a_file_is_not_empty() {
    let (provider, vfs) = mount(Options::default());
    let docs = provider.add_folder(&provider.root(), "docs");
    provider.add_file(&docs, "a.txt", b"a");
    let folder = vfs.lookup(Vfs::ROOT, "docs").await.unwrap();
    assert_eq!(
        vfs.remove(Vfs::ROOT, "docs", Kind::Folder).await,
        Err(VfsError::NotEmpty)
    );
    assert_eq!(
        vfs.lookup(Vfs::ROOT, "docs").await.unwrap().node,
        folder.node
    );
    assert_eq!(
        vfs.lookup(folder.node, "a.txt").await.unwrap().name,
        "a.txt"
    );
}

#[tokio::test]
async fn remove_of_a_missing_name_is_not_found() {
    let (provider, vfs) = mount(Options::default());
    provider.add_file(&provider.root(), "a.txt", b"a");
    vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    assert_eq!(
        vfs.remove(Vfs::ROOT, "missing", Kind::File).await,
        Err(VfsError::NotFound)
    );
}

#[tokio::test]
async fn remove_of_an_item_another_client_moved_is_not_found() {
    let (provider, vfs) = mount(Options::default());
    let root = provider.root();
    let folder_a = provider.add_folder(&root, "a");
    let folder_b = provider.add_folder(&root, "b");
    let file = provider.add_file(&folder_a, "note.txt", b"body");
    let source = vfs.lookup(Vfs::ROOT, "a").await.unwrap();
    vfs.lookup(source.node, "note.txt").await.unwrap();
    provider.remote_rename(&file, &folder_b, "moved.txt");
    assert_eq!(
        vfs.remove(source.node, "note.txt", Kind::File).await,
        Err(VfsError::NotFound)
    );
    let destination = vfs.lookup(Vfs::ROOT, "b").await.unwrap();
    assert_eq!(
        vfs.lookup(destination.node, "moved.txt")
            .await
            .unwrap()
            .name,
        "moved.txt"
    );
}

#[tokio::test]
async fn a_rename_in_one_folder_keeps_the_node() {
    let (provider, vfs) = mount(Options::default());
    provider.add_file(&provider.root(), "note.txt", b"body");
    let note = vfs.lookup(Vfs::ROOT, "note.txt").await.unwrap();
    let calls = provider.calls();
    vfs.rename(
        Vfs::ROOT,
        "note.txt",
        Vfs::ROOT,
        "moved.txt",
        IfExists::Replace,
    )
    .await
    .unwrap();
    assert_eq!(
        vfs.lookup(Vfs::ROOT, "note.txt").await,
        Err(VfsError::NotFound)
    );
    let moved = vfs.lookup(Vfs::ROOT, "moved.txt").await.unwrap();
    assert_eq!(moved.node, note.node);
    assert_eq!(provider.calls(), calls + 1);
}

#[tokio::test]
async fn a_move_to_another_folder_keeps_the_node() {
    let (provider, vfs) = mount(Options::default());
    let root = provider.root();
    let folder_a = provider.add_folder(&root, "a");
    provider.add_folder(&root, "b");
    provider.add_file(&folder_a, "note.txt", b"body");
    let source = vfs.lookup(Vfs::ROOT, "a").await.unwrap();
    let destination = vfs.lookup(Vfs::ROOT, "b").await.unwrap();
    let note = vfs.lookup(source.node, "note.txt").await.unwrap();
    vfs.lookup(destination.node, "missing").await.unwrap_err();
    let calls = provider.calls();
    vfs.rename(
        source.node,
        "note.txt",
        destination.node,
        "moved.txt",
        IfExists::Replace,
    )
    .await
    .unwrap();
    assert_eq!(
        vfs.lookup(source.node, "note.txt").await,
        Err(VfsError::NotFound)
    );
    let moved = vfs.lookup(destination.node, "moved.txt").await.unwrap();
    assert_eq!(moved.node, note.node);
    assert_eq!(provider.calls(), calls + 1);
}

#[tokio::test]
async fn rename_replaces_the_item_that_holds_the_name() {
    let (provider, vfs) = mount(Options::default());
    provider.add_file(&provider.root(), "a.txt", b"aaa");
    provider.add_file(&provider.root(), "b.txt", b"bbb");
    let source = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    vfs.lookup(Vfs::ROOT, "b.txt").await.unwrap();
    vfs.rename(Vfs::ROOT, "a.txt", Vfs::ROOT, "b.txt", IfExists::Replace)
        .await
        .unwrap();
    assert_eq!(
        vfs.lookup(Vfs::ROOT, "a.txt").await,
        Err(VfsError::NotFound)
    );
    let moved = vfs.lookup(Vfs::ROOT, "b.txt").await.unwrap();
    assert_eq!(moved.node, source.node);
    let handle = vfs.open_folder(Vfs::ROOT).await.unwrap();
    let entries = vfs.read_folder(handle, 0, 10).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "b.txt");
    assert_eq!(entries[0].node, source.node);
}

#[tokio::test]
async fn rename_can_refuse_a_taken_name() {
    let (provider, vfs) = mount(Options::default());
    provider.add_file(&provider.root(), "a.txt", b"aaa");
    provider.add_file(&provider.root(), "b.txt", b"bbb");
    let source = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let target = vfs.lookup(Vfs::ROOT, "b.txt").await.unwrap();
    let calls = provider.calls();
    assert_eq!(
        vfs.rename(Vfs::ROOT, "a.txt", Vfs::ROOT, "b.txt", IfExists::Refuse)
            .await,
        Err(VfsError::Conflict)
    );
    assert_eq!(provider.calls(), calls);
    assert_eq!(
        vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap().node,
        source.node
    );
    assert_eq!(
        vfs.lookup(Vfs::ROOT, "b.txt").await.unwrap().node,
        target.node
    );
}

#[tokio::test]
async fn a_folder_renamed_onto_a_file_is_not_a_folder() {
    let (provider, vfs) = mount(Options::default());
    provider.add_folder(&provider.root(), "docs");
    provider.add_file(&provider.root(), "a.txt", b"a");
    let folder = vfs.lookup(Vfs::ROOT, "docs").await.unwrap();
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    assert_eq!(
        vfs.rename(Vfs::ROOT, "docs", Vfs::ROOT, "a.txt", IfExists::Replace)
            .await,
        Err(VfsError::NotAFolder)
    );
    assert_eq!(
        vfs.lookup(Vfs::ROOT, "docs").await.unwrap().node,
        folder.node
    );
    assert_eq!(
        vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap().node,
        file.node
    );
}

#[tokio::test]
async fn a_file_renamed_onto_a_folder_is_a_folder() {
    let (provider, vfs) = mount(Options::default());
    provider.add_folder(&provider.root(), "docs");
    provider.add_file(&provider.root(), "a.txt", b"a");
    let folder = vfs.lookup(Vfs::ROOT, "docs").await.unwrap();
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    assert_eq!(
        vfs.rename(Vfs::ROOT, "a.txt", Vfs::ROOT, "docs", IfExists::Replace)
            .await,
        Err(VfsError::IsAFolder)
    );
    assert_eq!(
        vfs.lookup(Vfs::ROOT, "docs").await.unwrap().node,
        folder.node
    );
    assert_eq!(
        vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap().node,
        file.node
    );
}

#[tokio::test]
async fn a_folder_renamed_onto_a_full_folder_is_not_empty() {
    let (provider, vfs) = mount(Options::default());
    let full = provider.add_folder(&provider.root(), "full");
    provider.add_file(&full, "inside.txt", b"x");
    provider.add_folder(&provider.root(), "empty");
    let full_node = vfs.lookup(Vfs::ROOT, "full").await.unwrap();
    vfs.lookup(Vfs::ROOT, "empty").await.unwrap();
    assert_eq!(
        vfs.rename(Vfs::ROOT, "empty", Vfs::ROOT, "full", IfExists::Replace)
            .await,
        Err(VfsError::NotEmpty)
    );
    assert!(vfs.lookup(Vfs::ROOT, "empty").await.is_ok());
    assert_eq!(
        vfs.lookup(Vfs::ROOT, "full").await.unwrap().node,
        full_node.node
    );
    assert_eq!(
        vfs.lookup(full_node.node, "inside.txt").await.unwrap().name,
        "inside.txt"
    );
}

#[tokio::test]
async fn a_folder_moved_into_itself_is_not_supported() {
    let (provider, vfs) = mount(Options::default());
    provider.add_folder(&provider.root(), "docs");
    let folder = vfs.lookup(Vfs::ROOT, "docs").await.unwrap();
    assert_eq!(
        vfs.rename(Vfs::ROOT, "docs", folder.node, "inner", IfExists::Replace)
            .await,
        Err(VfsError::NotSupported)
    );
    assert_eq!(
        vfs.lookup(Vfs::ROOT, "docs").await.unwrap().node,
        folder.node
    );
    assert_eq!(
        vfs.lookup(folder.node, "inner").await,
        Err(VfsError::NotFound)
    );
}

#[tokio::test]
async fn a_rename_to_the_same_name_asks_the_provider_nothing() {
    let (provider, vfs) = mount(Options::default());
    provider.add_file(&provider.root(), "a.txt", b"a");
    vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let calls = provider.calls();
    vfs.rename(Vfs::ROOT, "a.txt", Vfs::ROOT, "a.txt", IfExists::Replace)
        .await
        .unwrap();
    assert_eq!(
        vfs.rename(Vfs::ROOT, "a.txt", Vfs::ROOT, "a.txt", IfExists::Refuse)
            .await,
        Err(VfsError::Conflict)
    );
    assert_eq!(provider.calls(), calls);
}

#[tokio::test]
async fn rename_does_not_replace_an_item_another_client_moved_away() {
    let (provider, vfs) = mount(Options::default());
    let root = provider.root();
    let archive = provider.add_folder(&root, "archive");
    provider.add_file(&root, "draft.txt", b"new");
    let report = provider.add_file(&root, "report.txt", b"old");
    vfs.lookup(Vfs::ROOT, "report.txt").await.unwrap();
    provider.remote_rename(&report, &archive, "report.txt");
    assert_eq!(
        vfs.rename(
            Vfs::ROOT,
            "draft.txt",
            Vfs::ROOT,
            "report.txt",
            IfExists::Replace
        )
        .await,
        Err(VfsError::NotFound)
    );
    let kept = provider.list(&archive).await.unwrap();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].id, report);
    assert_eq!(
        vfs.lookup(Vfs::ROOT, "draft.txt").await.unwrap().name,
        "draft.txt"
    );
}

#[tokio::test(start_paused = true)]
async fn a_change_that_waits_too_long_for_another_times_out() {
    let (provider, vfs) = mount(Options::default());
    // The first change holds the lock for 40 seconds: its listing takes 20, and so does its create. Neither passes the 30-second limit of one call.
    provider.set_delay(Duration::from_secs(20));
    let vfs_for_first = Arc::clone(&vfs);
    let first = tokio::spawn(async move { vfs_for_first.create_folder(Vfs::ROOT, "a").await });
    until_in_flight(&provider, 1).await;
    assert_eq!(
        vfs.create_folder(Vfs::ROOT, "b").await,
        Err(VfsError::TimedOut)
    );
    assert_eq!(first.await.unwrap().unwrap().name, "a");
    provider.set_delay(Duration::ZERO);
    let items = provider.list(&provider.root()).await.unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "a");
}

#[tokio::test]
async fn a_rename_that_changes_the_id_keeps_the_node_and_the_open_handle() {
    let (provider, vfs) = mount(Options::default());
    provider.rename_changes_ids(true);
    provider.add_file(&provider.root(), "a.txt", b"hello");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Read).await.unwrap();
    vfs.rename(Vfs::ROOT, "a.txt", Vfs::ROOT, "b.txt", IfExists::Replace)
        .await
        .unwrap();
    let moved = vfs.lookup(Vfs::ROOT, "b.txt").await.unwrap();
    assert_eq!(moved.node, file.node);
    assert_eq!(vfs.attributes(file.node).await.unwrap().size, 5);
    assert_eq!(vfs.read(handle, 0, 5).await.unwrap(), b"hello");
}

#[tokio::test]
async fn a_folder_whose_id_changes_still_lists_what_it_holds() {
    let (provider, vfs) = mount(Options::default());
    provider.rename_changes_ids(true);
    let docs = provider.add_folder(&provider.root(), "docs");
    provider.add_file(&docs, "a.txt", b"a");
    let folder = vfs.lookup(Vfs::ROOT, "docs").await.unwrap();
    vfs.rename(Vfs::ROOT, "docs", Vfs::ROOT, "papers", IfExists::Replace)
        .await
        .unwrap();
    assert_eq!(
        vfs.lookup(folder.node, "a.txt").await.unwrap().name,
        "a.txt"
    );
}

#[tokio::test]
async fn a_failed_change_drops_the_cached_listing() {
    let (provider, vfs) = mount(Options::default());
    provider.add_file(&provider.root(), "a.txt", b"a");
    vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let calls = provider.calls();
    provider.fail_with(Some(VfsError::Offline));
    assert_eq!(
        vfs.create_folder(Vfs::ROOT, "docs").await,
        Err(VfsError::Offline)
    );
    provider.fail_with(None);
    assert_eq!(vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap().name, "a.txt");
    assert_eq!(provider.calls(), calls + 2);
}

#[tokio::test(start_paused = true)]
async fn a_cancelled_create_is_found_on_the_next_lookup() {
    let (provider, vfs) = mount(Options::default());
    vfs.lookup(Vfs::ROOT, "missing").await.unwrap_err();
    provider.set_delay(Duration::from_secs(60));
    let vfs_for_create = Arc::clone(&vfs);
    let task = tokio::spawn(async move { vfs_for_create.create_folder(Vfs::ROOT, "docs").await });
    until_in_flight(&provider, 1).await;
    task.abort();
    let _ = task.await;
    provider.set_delay(Duration::ZERO);
    assert_eq!(vfs.lookup(Vfs::ROOT, "docs").await.unwrap().name, "docs");
}

#[tokio::test(start_paused = true)]
async fn a_listing_in_flight_does_not_hide_a_new_folder() {
    let (provider, vfs) = mount(Options::default());
    provider.add_file(&provider.root(), "a.txt", b"a");
    provider.set_delay(Duration::from_secs(3));
    let vfs_for_lookup = Arc::clone(&vfs);
    let first = tokio::spawn(async move { vfs_for_lookup.lookup(Vfs::ROOT, "a.txt").await });
    until_in_flight(&provider, 1).await;
    provider.set_delay(Duration::ZERO);
    let created = vfs.create_folder(Vfs::ROOT, "docs").await.unwrap();
    first.await.unwrap().unwrap();
    let calls = provider.calls();
    let found = vfs.lookup(Vfs::ROOT, "docs").await.unwrap();
    assert_eq!(found.node, created.node);
    assert_eq!(provider.calls(), calls);
}

#[tokio::test(start_paused = true)]
async fn a_listing_that_already_holds_the_new_folder_shows_it_once() {
    let (provider, vfs) = mount(Options::default());
    provider.add_file(&provider.root(), "other.txt", b"o");
    vfs.lookup(Vfs::ROOT, "other.txt").await.unwrap();
    provider.set_delay(Duration::from_secs(1));
    let vfs_for_create = Arc::clone(&vfs);
    let task = tokio::spawn(async move { vfs_for_create.create_folder(Vfs::ROOT, "docs").await });
    until_in_flight(&provider, 1).await;
    vfs.refresh();
    provider.set_delay(Duration::ZERO);
    vfs.lookup(Vfs::ROOT, "other.txt").await.unwrap();
    task.await.unwrap().unwrap();
    let handle = vfs.open_folder(Vfs::ROOT).await.unwrap();
    let entries = vfs.read_folder(handle, 0, 10).unwrap();
    let docs: Vec<_> = entries
        .iter()
        .filter(|entry| entry.name == "docs" || entry.name.starts_with("docs "))
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(docs, ["docs"]);
}

#[tokio::test(start_paused = true)]
async fn two_creates_of_one_name_make_one_folder() {
    let (provider, vfs) = mount(Options::default());
    vfs.lookup(Vfs::ROOT, "missing").await.unwrap_err();
    provider.set_delay(Duration::from_secs(1));
    let vfs_for_first = Arc::clone(&vfs);
    let first = tokio::spawn(async move { vfs_for_first.create_folder(Vfs::ROOT, "docs").await });
    until_in_flight(&provider, 1).await;
    let vfs_for_second = Arc::clone(&vfs);
    let second = tokio::spawn(async move { vfs_for_second.create_folder(Vfs::ROOT, "docs").await });
    assert!(first.await.unwrap().is_ok());
    assert_eq!(second.await.unwrap(), Err(VfsError::Conflict));
    provider.set_delay(Duration::ZERO);
    let items = provider.list(&provider.root()).await.unwrap();
    assert_eq!(items.iter().filter(|item| item.name == "docs").count(), 1);
}

/// A provider that leaves the three change calls out.
struct ReadOnlyProvider;

#[async_trait::async_trait]
impl Provider for ReadOnlyProvider {
    fn root(&self) -> ItemId {
        ItemId("root".to_string())
    }

    async fn item(&self, _id: &ItemId) -> Result<Item, VfsError> {
        Err(VfsError::NotFound)
    }

    async fn list(&self, _folder: &ItemId) -> Result<Vec<Item>, VfsError> {
        Ok(vec![Item {
            id: ItemId("file".to_string()),
            name: "a.txt".to_string(),
            attributes: Attributes {
                kind: Kind::File,
                size: 0,
                modified: UNIX_EPOCH,
            },
        }])
    }

    async fn read(&self, _id: &ItemId, _offset: u64, _length: u32) -> Result<Vec<u8>, VfsError> {
        Err(VfsError::NotFound)
    }

    async fn space(&self) -> Result<Space, VfsError> {
        Ok(Space { total: 0, free: 0 })
    }
}

#[tokio::test]
async fn a_provider_without_changes_is_read_only() {
    let vfs = Vfs::new(Arc::new(ReadOnlyProvider), POSIX, Options::default());
    assert_eq!(
        vfs.create_folder(Vfs::ROOT, "docs").await,
        Err(VfsError::ReadOnly)
    );
    assert_eq!(
        vfs.remove(Vfs::ROOT, "a.txt", Kind::File).await,
        Err(VfsError::ReadOnly)
    );
    assert_eq!(
        vfs.rename(Vfs::ROOT, "a.txt", Vfs::ROOT, "b.txt", IfExists::Replace)
            .await,
        Err(VfsError::ReadOnly)
    );
}
