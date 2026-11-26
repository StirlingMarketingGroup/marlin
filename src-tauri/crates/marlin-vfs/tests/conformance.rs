use std::collections::HashSet;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use marlin_vfs::{
    finder_files, Access, Attributes, Copied, IfExists, Item, ItemId, Kind, MemoryProvider, NodeId,
    Options, Provider, Space, Spool, UnsentState, Vfs, VfsError, POSIX,
};

fn never_local(_name: &str) -> bool {
    false
}

fn spooled_mount(
    options: Options,
    limit: u64,
    stays_local: fn(&str) -> bool,
) -> (Arc<MemoryProvider>, Arc<Vfs>, tempfile::TempDir) {
    spooled_with(MemoryProvider::new(), options, limit, stays_local)
}

fn spooled_path_mount(
    options: Options,
    limit: u64,
    stays_local: fn(&str) -> bool,
) -> (Arc<MemoryProvider>, Arc<Vfs>, tempfile::TempDir) {
    spooled_with(MemoryProvider::with_path_ids(), options, limit, stays_local)
}

fn spooled_with(
    provider: MemoryProvider,
    options: Options,
    limit: u64,
    stays_local: fn(&str) -> bool,
) -> (Arc<MemoryProvider>, Arc<Vfs>, tempfile::TempDir) {
    let provider = Arc::new(provider);
    let dir = tempfile::tempdir().unwrap();
    let vfs = mount_on(&provider, &dir, options, limit, stays_local);
    (provider, vfs, dir)
}

/// A second mount on a provider and a spool folder a test already has.
fn mount_on(
    provider: &Arc<MemoryProvider>,
    dir: &tempfile::TempDir,
    options: Options,
    limit: u64,
    stays_local: fn(&str) -> bool,
) -> Arc<Vfs> {
    Arc::new(
        Vfs::new(provider.clone(), POSIX, options)
            .take_writes(Spool {
                folder: dir.path().to_path_buf(),
                limit,
                stays_local,
            })
            .unwrap(),
    )
}

fn spool_files(dir: &tempfile::TempDir) -> usize {
    std::fs::read_dir(dir.path())
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .unwrap()
                .file_name()
                .to_str()
                .is_some_and(|name| name.parse::<u64>().is_ok())
        })
        .count()
}

/// Every name in the spool folder, records included.
fn spool_entries(dir: &tempfile::TempDir) -> usize {
    std::fs::read_dir(dir.path()).unwrap().count()
}

fn mount(options: Options) -> (Arc<MemoryProvider>, Arc<Vfs>) {
    mounted(MemoryProvider::new(), options)
}

fn path_mount(options: Options) -> (Arc<MemoryProvider>, Arc<Vfs>) {
    mounted(MemoryProvider::with_path_ids(), options)
}

fn mounted(provider: MemoryProvider, options: Options) -> (Arc<MemoryProvider>, Arc<Vfs>) {
    let provider = Arc::new(provider);
    let vfs = Arc::new(Vfs::new(provider.clone(), POSIX, options));
    (provider, vfs)
}

fn path(text: &str) -> ItemId {
    ItemId(text.to_string())
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

/// Polls until `ready` without moving the clock, so a task woken at this instant gets a turn.
///
/// A try syncs the spool file on a blocking thread before its byte call. Each miss waits a millisecond of wall time for that thread. The paused clock stays where it is.
async fn yield_until(what: &str, mut ready: impl FnMut() -> bool) {
    for _ in 0..1_000 {
        if ready() {
            return;
        }
        tokio::task::yield_now().await;
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("{what}");
}

/// Moves the paused clock by `wait`, then lets tasks woken at that instant run.
async fn advance(wait: Duration) {
    tokio::time::sleep(wait).await;
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
}

fn settle() -> Duration {
    Options::default().settle
}

async fn root_items(provider: &MemoryProvider) -> Vec<Item> {
    provider.list(&provider.root()).await.unwrap()
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
    let handle = vfs.open_folder(Vfs::ROOT).unwrap();
    vfs.rewind_folder(handle).await.unwrap();
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
async fn rewind_folder_takes_a_new_listing() {
    let (provider, vfs) = mount(Options::default());
    let removed_id = provider.add_file(&provider.root(), "a.txt", b"a");
    provider.add_file(&provider.root(), "b.txt", b"b");
    let handle = vfs.open_folder(Vfs::ROOT).unwrap();
    vfs.rewind_folder(handle).await.unwrap();
    let opened = vfs.read_folder(handle, 0, 10).unwrap();
    let names = |entries: &[marlin_vfs::Entry]| {
        entries
            .iter()
            .map(|entry| entry.name.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(names(&opened), ["a.txt", "b.txt"]);
    let removed = opened
        .iter()
        .find(|entry| entry.name == "a.txt")
        .unwrap()
        .node;
    provider.add_file(&provider.root(), "c.txt", b"c");
    provider.remote_remove(&removed_id);
    vfs.refresh();
    assert_eq!(
        names(&vfs.read_folder(handle, 0, 10).unwrap()),
        ["a.txt", "b.txt"]
    );
    vfs.rewind_folder(handle).await.unwrap();
    assert_eq!(
        names(&vfs.read_folder(handle, 0, 10).unwrap()),
        ["b.txt", "c.txt"]
    );
    // The rewind took off the removed item's last reference.
    assert_eq!(vfs.attributes(removed).await, Err(VfsError::StaleHandle));
    vfs.release(handle);
    assert_eq!(vfs.rewind_folder(handle).await, Err(VfsError::StaleHandle));
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
    assert_eq!(vfs.open_folder(file.node), Err(VfsError::NotAFolder));
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
    let handle = vfs.open_folder(Vfs::ROOT).unwrap();
    vfs.rewind_folder(handle).await.unwrap();
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
    let handle = vfs.open_folder(folder.node).unwrap();
    vfs.rewind_folder(handle).await.unwrap();
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
    let handle = vfs.open_folder(Vfs::ROOT).unwrap();
    vfs.rewind_folder(handle).await.unwrap();
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
    let handle = vfs.open_folder(Vfs::ROOT).unwrap();
    vfs.rewind_folder(handle).await.unwrap();
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
    let handle = vfs.open_folder(Vfs::ROOT).unwrap();
    vfs.rewind_folder(handle).await.unwrap();
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
    let (provider, vfs) = path_mount(Options::default());
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
    let (provider, vfs) = path_mount(Options::default());
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
    let handle = vfs.open_folder(Vfs::ROOT).unwrap();
    vfs.rewind_folder(handle).await.unwrap();
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
                revision: None,
                read_only: false,
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

#[tokio::test]
async fn writable_needs_a_spool_and_a_provider_that_can_write() {
    let (_provider, vfs) = mount(Options::default());
    assert!(!vfs.writable());
    let (_provider, vfs, _dir) = spooled_mount(Options::default(), 1024, never_local);
    assert!(vfs.writable());
    let dir = tempfile::tempdir().unwrap();
    let vfs = Vfs::new(Arc::new(ReadOnlyProvider), POSIX, Options::default())
        .take_writes(Spool {
            folder: dir.path().to_path_buf(),
            limit: 1024,
            stays_local: never_local,
        })
        .unwrap();
    assert!(!vfs.writable());
}

#[tokio::test]
async fn new_file_operations_reuse_cached_listings() {
    let (provider, vfs, _dir) = spooled_mount(Options::default(), 1024 * 1024, never_local);
    provider.add_folder(&provider.root(), "docs");
    provider.add_folder(&provider.root(), "more");
    let docs = vfs.lookup(Vfs::ROOT, "docs").await.unwrap();
    let more = vfs.lookup(Vfs::ROOT, "more").await.unwrap();
    assert_eq!(
        vfs.lookup(docs.node, "missing").await,
        Err(VfsError::NotFound)
    );
    assert_eq!(
        vfs.lookup(more.node, "missing").await,
        Err(VfsError::NotFound)
    );
    let calls = provider.calls();
    let (_made, handle) = vfs.create(docs.node, "a.txt", Access::Write).await.unwrap();
    vfs.write(handle, 0, b"ab").await.unwrap();
    vfs.write(handle, 4, b"cd").await.unwrap();
    assert_eq!(vfs.read(handle, 0, 6).await.unwrap(), b"ab\0\0cd");
    let file = vfs.lookup(docs.node, "a.txt").await.unwrap();
    vfs.truncate(file.node, 3).await.unwrap();
    assert_eq!(vfs.read(handle, 0, 6).await.unwrap(), b"ab\0");
    vfs.rename(docs.node, "a.txt", docs.node, "b.txt", IfExists::Replace)
        .await
        .unwrap();
    vfs.rename(docs.node, "b.txt", more.node, "c.txt", IfExists::Replace)
        .await
        .unwrap();
    vfs.remove(more.node, "c.txt", Kind::File).await.unwrap();
    assert_eq!(provider.calls(), calls);
    assert_eq!(
        vfs.lookup(more.node, "c.txt").await,
        Err(VfsError::NotFound)
    );
}

#[tokio::test]
async fn a_write_in_the_middle_of_a_large_file_fetches_it_in_pieces() {
    let (provider, vfs, _dir) = spooled_mount(Options::default(), 1024 * 1024 * 1024, never_local);
    let remote = vec![7u8; 50 * 1024 * 1024];
    let id = provider.add_file(&provider.root(), "big.bin", &remote);
    let file = vfs.lookup(Vfs::ROOT, "big.bin").await.unwrap();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    vfs.write(handle, 1000, b"XYZ").await.unwrap();
    let read = vfs.open(file.node, Access::Read).await.unwrap();
    let got = vfs.read(read, 0, remote.len() as u32).await.unwrap();
    let mut expect = remote.clone();
    expect[1000..1003].copy_from_slice(b"XYZ");
    assert_eq!(got, expect);
    assert_eq!(provider.contents(&id), remote);
    assert!(provider.largest_read() <= 4 * 1024 * 1024);
}

#[tokio::test]
async fn opening_for_write_and_closing_fetches_nothing() {
    let (provider, vfs, _dir) = spooled_mount(Options::default(), 1024 * 1024, never_local);
    provider.add_file(&provider.root(), "a.txt", b"hello");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let calls = provider.calls();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    vfs.release(handle);
    assert_eq!(provider.calls(), calls);
    let read = vfs.open(file.node, Access::Read).await.unwrap();
    assert_eq!(vfs.read(read, 0, 5).await.unwrap(), b"hello");
    assert!(provider.calls() > calls);
}

#[tokio::test]
async fn a_spooled_file_shows_the_spool_size_and_modified_time() {
    let (provider, vfs, _dir) = spooled_mount(Options::default(), 1024 * 1024, never_local);
    provider.add_file(&provider.root(), "a.txt", b"hello");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let remote_modified = file.attributes.modified;
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    vfs.write(handle, 0, b"HELLO").await.unwrap();
    let (made, new_handle) = vfs.create(Vfs::ROOT, "b.txt", Access::Write).await.unwrap();
    vfs.write(new_handle, 0, b"xyz").await.unwrap();
    let calls = provider.calls();
    let looked = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let looked_new = vfs.lookup(Vfs::ROOT, "b.txt").await.unwrap();
    let folder = vfs.open_folder(Vfs::ROOT).unwrap();
    vfs.rewind_folder(folder).await.unwrap();
    let entries = vfs.read_folder(folder, 0, 10).unwrap();
    let listed = entries.iter().find(|entry| entry.name == "a.txt").unwrap();
    let listed_new = entries.iter().find(|entry| entry.name == "b.txt").unwrap();
    let attrs = vfs.attributes(file.node).await.unwrap();
    let attrs_new = vfs.attributes(made.node).await.unwrap();
    assert_eq!(provider.calls(), calls);
    assert_eq!(looked.attributes, attrs);
    assert_eq!(listed.attributes, attrs);
    assert_eq!(attrs.size, 5);
    assert_ne!(attrs.modified, remote_modified);
    assert_eq!(looked_new.attributes, attrs_new);
    assert_eq!(listed_new.attributes, attrs_new);
    assert_eq!(attrs_new.size, 3);
}

#[tokio::test]
async fn a_new_file_renamed_over_an_existing_file_takes_its_id() {
    let (provider, vfs, _dir) = spooled_mount(Options::default(), 1024 * 1024, never_local);
    let old_id = provider.add_file(&provider.root(), "old.txt", b"old-bytes");
    let old = vfs.lookup(Vfs::ROOT, "old.txt").await.unwrap();
    let old_handle = vfs.open(old.node, Access::Read).await.unwrap();
    // The OS asks for the root before it makes a file there. A new file then asks the provider nothing.
    vfs.attributes(Vfs::ROOT).await.unwrap();
    let calls = provider.calls();
    let (made, handle) = vfs
        .create(Vfs::ROOT, "new.txt", Access::Write)
        .await
        .unwrap();
    vfs.write(handle, 0, b"new-bytes").await.unwrap();
    vfs.rename(
        Vfs::ROOT,
        "new.txt",
        Vfs::ROOT,
        "old.txt",
        IfExists::Replace,
    )
    .await
    .unwrap();
    assert_eq!(provider.calls(), calls);
    let folder = vfs.open_folder(Vfs::ROOT).unwrap();
    vfs.rewind_folder(folder).await.unwrap();
    let names: Vec<_> = vfs
        .read_folder(folder, 0, 10)
        .unwrap()
        .into_iter()
        .map(|entry| entry.name)
        .collect();
    assert_eq!(names, ["old.txt"]);
    let found = vfs.lookup(Vfs::ROOT, "old.txt").await.unwrap();
    assert_eq!(found.node, made.node);
    let read = vfs.open(found.node, Access::Read).await.unwrap();
    assert_eq!(vfs.read(read, 0, 20).await.unwrap(), b"new-bytes");
    // The replaced file's own handle keeps its old bytes, as with a POSIX rename.
    assert_eq!(vfs.read(old_handle, 0, 20).await.unwrap(), b"old-bytes");
    assert_eq!(provider.contents(&old_id), b"old-bytes");
    // The renamed file is tied to the old item's id: it follows the item when another client moves it.
    provider.remote_rename(&old_id, &provider.root(), "moved.txt");
    vfs.refresh();
    assert_eq!(
        vfs.lookup(Vfs::ROOT, "moved.txt").await.unwrap().node,
        made.node
    );
}

#[tokio::test(start_paused = true)]
async fn a_write_past_the_limit_waits_then_keeps_the_earlier_bytes() {
    let call_timeout = Duration::from_secs(5);
    let (_provider, vfs, _dir) = spooled_mount(
        Options {
            call_timeout,
            ..Options::default()
        },
        4,
        never_local,
    );
    let (_first, first) = vfs.create(Vfs::ROOT, "a.txt", Access::Write).await.unwrap();
    vfs.write(first, 0, b"aa").await.unwrap();
    let (_second, second) = vfs.create(Vfs::ROOT, "b.txt", Access::Write).await.unwrap();
    vfs.write(second, 0, b"bb").await.unwrap();
    let started = tokio::time::Instant::now();
    assert_eq!(vfs.write(second, 2, b"c").await, Err(VfsError::NoSpace));
    assert!(started.elapsed() >= call_timeout);
    assert_eq!(vfs.read(second, 0, 10).await.unwrap(), b"bb");
    assert_eq!(
        vfs.write(first, 0, b"too long").await,
        Err(VfsError::NoSpace)
    );
    assert_eq!(started.elapsed(), call_timeout);
}

#[tokio::test(start_paused = true)]
async fn a_write_waiting_for_room_goes_on_when_another_file_is_removed() {
    let (_provider, vfs, _dir) = spooled_mount(Options::default(), 10, never_local);
    let (other, other_handle) = vfs.create(Vfs::ROOT, "b.txt", Access::Write).await.unwrap();
    vfs.write(other_handle, 0, b"bbbbbb").await.unwrap();
    let (_made, handle) = vfs.create(Vfs::ROOT, "a.txt", Access::Write).await.unwrap();
    vfs.write(handle, 0, b"aaaa").await.unwrap();
    let mut waiting = std::pin::pin!(vfs.write(handle, 4, b"z"));
    let first = std::future::poll_fn(|cx| std::task::Poll::Ready(waiting.as_mut().poll(cx))).await;
    assert!(first.is_pending(), "the write waits for room");
    vfs.remove(Vfs::ROOT, "b.txt", Kind::File).await.unwrap();
    vfs.release(other_handle);
    vfs.forget(other.node, 1);
    waiting.await.unwrap();
    assert_eq!(vfs.read(handle, 0, 5).await.unwrap(), b"aaaaz");
}

#[tokio::test]
async fn finder_files_stay_on_this_computer_and_a_real_file_stays_in_the_spool() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024 * 1024, finder_files);
    let photo = provider.add_file(&provider.root(), "._photo.jpg", b"remote-bytes");
    let (made, handle) = vfs
        .create(Vfs::ROOT, ".DS_Store", Access::Write)
        .await
        .unwrap();
    vfs.write(handle, 0, b"one").await.unwrap();
    assert_eq!(vfs.read(handle, 0, 3).await.unwrap(), b"one");
    vfs.remove(Vfs::ROOT, ".DS_Store", Kind::File)
        .await
        .unwrap();
    vfs.release(handle);
    vfs.forget(made.node, 1);
    assert_eq!(spool_files(&dir), 0);
    let (_again, again) = vfs
        .create(Vfs::ROOT, ".DS_Store", Access::Write)
        .await
        .unwrap();
    vfs.write(again, 0, b"two").await.unwrap();
    let (_notes, notes) = vfs
        .create(Vfs::ROOT, "notes.txt", Access::Write)
        .await
        .unwrap();
    vfs.write(notes, 0, b"keep").await.unwrap();
    let photo_node = vfs.lookup(Vfs::ROOT, "._photo.jpg").await.unwrap();
    let photo_handle = vfs.open(photo_node.node, Access::Write).await.unwrap();
    vfs.write(photo_handle, 0, b"local").await.unwrap();
    assert_eq!(provider.contents(&photo), b"remote-bytes");
    drop(vfs);
    let mut names: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_str().unwrap().to_string())
        .collect();
    names.sort();
    let kept = names
        .iter()
        .find(|name| name.parse::<u64>().is_ok())
        .unwrap();
    assert_eq!(std::fs::read(dir.path().join(kept)).unwrap(), b"keep");
    assert_eq!(names.len(), 2);
    assert!(names.contains(&format!("{kept}.record")));
}

#[tokio::test]
async fn a_removed_new_file_with_an_open_handle_keeps_its_bytes_until_the_node_goes() {
    let (_provider, vfs, dir) = spooled_mount(Options::default(), 1024 * 1024, never_local);
    let (made, handle) = vfs.create(Vfs::ROOT, "a.txt", Access::Write).await.unwrap();
    vfs.write(handle, 0, b"hello").await.unwrap();
    vfs.remove(Vfs::ROOT, "a.txt", Kind::File).await.unwrap();
    assert_eq!(vfs.read(handle, 0, 5).await.unwrap(), b"hello");
    vfs.write(handle, 0, b"HELLO").await.unwrap();
    assert_eq!(vfs.read(handle, 0, 5).await.unwrap(), b"HELLO");
    let folder = vfs.open_folder(Vfs::ROOT).unwrap();
    vfs.rewind_folder(folder).await.unwrap();
    assert!(vfs.read_folder(folder, 0, 10).unwrap().is_empty());
    assert_eq!(spool_files(&dir), 1);
    vfs.release(handle);
    vfs.forget(made.node, 1);
    assert_eq!(spool_files(&dir), 0);
}

#[tokio::test]
async fn a_failed_fetch_leaves_the_file_clean() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024 * 1024, never_local);
    provider.add_file(&provider.root(), "a.txt", b"hello");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    provider.fail_with(Some(VfsError::Offline));
    assert_eq!(vfs.write(handle, 0, b"Z").await, Err(VfsError::Offline));
    assert_eq!(spool_files(&dir), 0);
    provider.fail_with(None);
    let calls = provider.calls();
    let read = vfs.open(file.node, Access::Read).await.unwrap();
    assert_eq!(vfs.read(read, 0, 5).await.unwrap(), b"hello");
    assert!(provider.calls() > calls);
    vfs.write(handle, 0, b"Z").await.unwrap();
    assert_eq!(vfs.read(read, 0, 5).await.unwrap(), b"Zello");
}

#[tokio::test]
async fn truncating_a_clean_file_to_zero_reads_nothing() {
    let (provider, vfs, _dir) = spooled_mount(Options::default(), 1024 * 1024, never_local);
    provider.add_file(&provider.root(), "a.txt", b"hello");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let calls = provider.calls();
    vfs.truncate(file.node, 0).await.unwrap();
    assert_eq!(provider.calls(), calls);
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    vfs.write(handle, 0, b"xy").await.unwrap();
    assert_eq!(provider.calls(), calls);
    assert_eq!(vfs.read(handle, 0, 10).await.unwrap(), b"xy");
}

#[tokio::test]
async fn removing_a_folder_that_holds_a_new_file_is_not_empty() {
    let (provider, vfs, _dir) = spooled_mount(Options::default(), 1024 * 1024, never_local);
    provider.add_folder(&provider.root(), "docs");
    let docs = vfs.lookup(Vfs::ROOT, "docs").await.unwrap();
    vfs.lookup(docs.node, "missing").await.unwrap_err();
    vfs.create(docs.node, "a.txt", Access::Write).await.unwrap();
    let calls = provider.calls();
    assert_eq!(
        vfs.remove(Vfs::ROOT, "docs", Kind::Folder).await,
        Err(VfsError::NotEmpty)
    );
    assert_eq!(provider.calls(), calls);
}

#[tokio::test]
async fn create_and_write_report_a_taken_name_a_missing_spool_and_a_read_handle() {
    let (provider, vfs, _dir) = spooled_mount(Options::default(), 1024 * 1024, never_local);
    provider.add_file(&provider.root(), "a.txt", b"hello");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    assert_eq!(
        vfs.create(Vfs::ROOT, "a.txt", Access::Write)
            .await
            .unwrap_err(),
        VfsError::Conflict
    );
    let no_spool = Vfs::new(provider.clone(), POSIX, Options::default());
    assert_eq!(
        no_spool.create(Vfs::ROOT, "b.txt", Access::Write).await,
        Err(VfsError::ReadOnly)
    );
    let handle = vfs.open(file.node, Access::Read).await.unwrap();
    assert_eq!(
        vfs.write(handle, 0, b"Z").await,
        Err(VfsError::AccessDenied)
    );
}

#[tokio::test]
async fn a_new_file_where_the_remote_takes_nothing_new_fails_at_once() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024 * 1024, finder_files);
    let shares_id = provider.add_folder(&provider.root(), "shares");
    provider.refuse_new_items(&shares_id, true);
    let shares = vfs.lookup(Vfs::ROOT, "shares").await.unwrap();
    assert_eq!(
        vfs.create(shares.node, "a.txt", Access::Write)
            .await
            .unwrap_err(),
        VfsError::NotSupported
    );
    assert_eq!(
        vfs.create_folder(shares.node, "b").await.unwrap_err(),
        VfsError::NotSupported
    );
    assert_eq!(spool_files(&dir), 0);
    assert!(vfs.unsent().is_empty());
    assert_eq!(
        vfs.lookup(shares.node, "a.txt").await,
        Err(VfsError::NotFound)
    );
    let (_made, handle) = vfs
        .create(shares.node, ".DS_Store", Access::Write)
        .await
        .unwrap();
    assert_eq!(
        vfs.rename(
            shares.node,
            ".DS_Store",
            shares.node,
            "notes.txt",
            IfExists::Refuse,
        )
        .await,
        Err(VfsError::NotSupported)
    );
    vfs.lookup(shares.node, ".DS_Store").await.unwrap();
    vfs.release(handle);
}

#[tokio::test]
async fn a_name_the_remote_cannot_hold_fails_at_once() {
    let (_provider, vfs, dir) = spooled_path_mount(Options::default(), 1024 * 1024, never_local);
    assert_eq!(
        vfs.create(Vfs::ROOT, "a%2Fb", Access::Write)
            .await
            .unwrap_err(),
        VfsError::NotSupported
    );
    assert_eq!(
        vfs.create_folder(Vfs::ROOT, "a%2Fb").await.unwrap_err(),
        VfsError::NotSupported
    );
    assert_eq!(spool_files(&dir), 0);
}

#[tokio::test(start_paused = true)]
async fn a_new_file_moved_where_the_remote_cannot_hold_it_stays_and_uploads() {
    let (provider, vfs, dir) = spooled_path_mount(Options::default(), 1024, never_local);
    let shares_id = provider.add_folder(&provider.root(), "shares");
    provider.refuse_new_items(&shares_id, true);
    let shares = vfs.lookup(Vfs::ROOT, "shares").await.unwrap();
    let (_made, handle) = vfs.create(Vfs::ROOT, "a.txt", Access::Write).await.unwrap();
    vfs.write(handle, 0, b"hello").await.unwrap();
    vfs.release(handle);
    assert_eq!(
        vfs.rename(Vfs::ROOT, "a.txt", shares.node, "a.txt", IfExists::Refuse)
            .await,
        Err(VfsError::NotSupported)
    );
    assert_eq!(
        vfs.rename(Vfs::ROOT, "a.txt", Vfs::ROOT, "a%2Fb.txt", IfExists::Refuse)
            .await,
        Err(VfsError::NotSupported)
    );
    vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let unsent = vfs.unsent();
    assert_eq!(unsent.len(), 1);
    assert_eq!(unsent[0].name, "a.txt");
    assert_eq!(unsent[0].folder, path("/"));
    let _uploads = start_uploads(&vfs);
    advance(settle()).await;
    until_spool_empty(&dir).await;
    let file = root_items(&provider)
        .await
        .into_iter()
        .find(|item| item.name == "a.txt")
        .unwrap();
    assert_eq!(provider.contents(&file.id), b"hello");
    assert!(provider.list(&shares_id).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_memory_rename_that_is_refused_replaces_nothing() {
    let provider = MemoryProvider::with_path_ids();
    provider.add_file(&provider.root(), "a.txt", b"a");
    provider.add_file(&provider.root(), "b.txt", b"b");
    assert_eq!(
        provider
            .rename(
                &path("/a.txt"),
                &path("/"),
                &path("/"),
                "x/y",
                Some(&path("/b.txt")),
            )
            .await,
        Err(VfsError::NotSupported)
    );
    assert!(provider.item(&path("/b.txt")).await.is_ok());
}

#[tokio::test]
async fn a_new_file_in_a_folder_the_account_may_not_add_to_fails_at_once() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024 * 1024, finder_files);
    let theirs_id = provider.add_folder(&provider.root(), "theirs");
    provider.set_read_only(&theirs_id, true);
    provider.add_file(&provider.root(), "r.txt", b"r");
    provider.add_file(&theirs_id, "in.txt", b"in");
    let theirs = vfs.lookup(Vfs::ROOT, "theirs").await.unwrap();
    assert!(theirs.attributes.read_only);
    assert_eq!(provider.check_new_item(&theirs_id, "a.txt"), Ok(()));
    // A remote item's move gets the provider's own answer, and a rename inside the folder is the provider's to allow.
    assert_eq!(
        vfs.rename(Vfs::ROOT, "r.txt", theirs.node, "r.txt", IfExists::Refuse)
            .await,
        Err(VfsError::AccessDenied)
    );
    vfs.lookup(Vfs::ROOT, "r.txt").await.unwrap();
    vfs.rename(
        theirs.node,
        "in.txt",
        theirs.node,
        "renamed.txt",
        IfExists::Refuse,
    )
    .await
    .unwrap();
    assert_eq!(
        vfs.create(theirs.node, "a.txt", Access::Write).await,
        Err(VfsError::AccessDenied)
    );
    assert_eq!(spool_files(&dir), 0);
    assert!(vfs.unsent().is_empty());
    assert_eq!(
        vfs.lookup(theirs.node, "a.txt").await,
        Err(VfsError::NotFound)
    );
    assert_eq!(
        vfs.create_folder(theirs.node, "b").await,
        Err(VfsError::AccessDenied)
    );
    let (_made, handle) = vfs
        .create(theirs.node, ".DS_Store", Access::Write)
        .await
        .unwrap();
    assert_eq!(
        vfs.rename(
            theirs.node,
            ".DS_Store",
            theirs.node,
            "notes.txt",
            IfExists::Refuse,
        )
        .await,
        Err(VfsError::AccessDenied)
    );
    vfs.lookup(theirs.node, ".DS_Store").await.unwrap();
    vfs.release(handle);
}

#[tokio::test(start_paused = true)]
async fn a_new_file_moved_into_a_folder_the_account_may_not_add_to_stays_and_uploads() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let theirs_id = provider.add_folder(&provider.root(), "theirs");
    provider.set_read_only(&theirs_id, true);
    let theirs = vfs.lookup(Vfs::ROOT, "theirs").await.unwrap();
    let (_made, handle) = vfs.create(Vfs::ROOT, "a.txt", Access::Write).await.unwrap();
    vfs.write(handle, 0, b"hello").await.unwrap();
    vfs.release(handle);
    assert_eq!(
        vfs.rename(Vfs::ROOT, "a.txt", theirs.node, "a.txt", IfExists::Refuse)
            .await,
        Err(VfsError::AccessDenied)
    );
    vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let unsent = vfs.unsent();
    assert_eq!(unsent.len(), 1);
    assert_eq!(unsent[0].name, "a.txt");
    assert_eq!(unsent[0].folder, provider.root());
    let _uploads = start_uploads(&vfs);
    advance(settle()).await;
    until_spool_empty(&dir).await;
    let file = root_items(&provider)
        .await
        .into_iter()
        .find(|item| item.name == "a.txt")
        .unwrap();
    assert_eq!(provider.contents(&file.id), b"hello");
    assert!(provider.list(&theirs_id).await.unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_new_file_renamed_over_a_file_in_a_folder_the_account_may_not_add_to_takes_its_place() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let theirs_id = provider.add_folder(&provider.root(), "theirs");
    provider.set_read_only(&theirs_id, true);
    let shared_id = provider.add_file(&theirs_id, "shared.txt", b"old");
    let theirs = vfs.lookup(Vfs::ROOT, "theirs").await.unwrap();
    let (_made, handle) = vfs
        .create(Vfs::ROOT, "tmp.txt", Access::Write)
        .await
        .unwrap();
    vfs.write(handle, 0, b"new").await.unwrap();
    vfs.release(handle);
    vfs.rename(
        Vfs::ROOT,
        "tmp.txt",
        theirs.node,
        "shared.txt",
        IfExists::Replace,
    )
    .await
    .unwrap();
    let _uploads = start_uploads(&vfs);
    advance(settle()).await;
    until_spool_empty(&dir).await;
    assert_eq!(provider.contents(&shared_id), b"new");
    let items = provider.list(&theirs_id).await.unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, shared_id);
}

#[tokio::test(start_paused = true)]
async fn the_root_is_asked_once_and_its_answer_decides_however_old() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024 * 1024, never_local);
    provider.set_read_only(&provider.root(), true);
    assert_eq!(
        vfs.create(Vfs::ROOT, "a.txt", Access::Write).await,
        Err(VfsError::AccessDenied)
    );
    // The remote's change reaches the core only with a new answer, and a new file does not ask for one.
    provider.set_read_only(&provider.root(), false);
    advance(Options::default().attributes_ttl).await;
    assert_eq!(
        vfs.create(Vfs::ROOT, "b.txt", Access::Write).await,
        Err(VfsError::AccessDenied)
    );
    assert_eq!(spool_files(&dir), 0);
}

#[tokio::test(start_paused = true)]
async fn a_new_file_in_a_folder_that_loses_the_right_after_its_answer_is_refused_at_upload() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let theirs_id = provider.add_folder(&provider.root(), "theirs");
    let theirs = vfs.lookup(Vfs::ROOT, "theirs").await.unwrap();
    // The core's answer still says the folder takes new items.
    provider.set_read_only(&theirs_id, true);
    let (_made, handle) = vfs
        .create(theirs.node, "a.txt", Access::Write)
        .await
        .unwrap();
    vfs.write(handle, 0, b"hello").await.unwrap();
    vfs.release(handle);
    let _uploads = start_uploads(&vfs);
    advance(settle()).await;
    yield_until("the upload was not refused", || {
        matches!(
            vfs.unsent().first().map(|unsent| unsent.state.clone()),
            Some(UnsentState::Refused(VfsError::AccessDenied))
        )
    })
    .await;
    assert_eq!(spool_files(&dir), 1);
    assert!(provider.list(&theirs_id).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_remote_item_that_takes_a_new_files_name_gets_a_suffix() {
    let (provider, vfs, _dir) = spooled_mount(Options::default(), 1024 * 1024, never_local);
    let (made, _handle) = vfs.create(Vfs::ROOT, "a.txt", Access::Write).await.unwrap();
    provider.add_file(&provider.root(), "a.txt", b"remote");
    vfs.refresh();
    let folder = vfs.open_folder(Vfs::ROOT).unwrap();
    vfs.rewind_folder(folder).await.unwrap();
    let entries = vfs.read_folder(folder, 0, 10).unwrap();
    let new_name = entries
        .iter()
        .find(|entry| entry.node == made.node)
        .unwrap()
        .name
        .clone();
    let remote_name = entries
        .iter()
        .find(|entry| entry.node != made.node)
        .unwrap()
        .name
        .clone();
    assert_eq!(new_name, "a.txt");
    assert_eq!(remote_name, "a (2).txt");
}

#[tokio::test]
async fn a_remote_file_renamed_onto_a_new_file_replaces_nothing() {
    let (provider, vfs, _dir) = spooled_mount(Options::default(), 1024 * 1024, never_local);
    let old_id = provider.add_file(&provider.root(), "old.txt", b"body");
    let old = vfs.lookup(Vfs::ROOT, "old.txt").await.unwrap();
    let (_made, _handle) = vfs
        .create(Vfs::ROOT, "new.txt", Access::Write)
        .await
        .unwrap();
    vfs.rename(
        Vfs::ROOT,
        "old.txt",
        Vfs::ROOT,
        "new.txt",
        IfExists::Replace,
    )
    .await
    .unwrap();
    let items = provider.list(&provider.root()).await.unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, old_id);
    assert_eq!(items[0].name, "new.txt");
    vfs.refresh();
    let found = vfs.lookup(Vfs::ROOT, "new.txt").await.unwrap();
    assert_eq!(found.node, old.node);
    let folder = vfs.open_folder(Vfs::ROOT).unwrap();
    vfs.rewind_folder(folder).await.unwrap();
    let names: Vec<_> = vfs
        .read_folder(folder, 0, 10)
        .unwrap()
        .into_iter()
        .map(|entry| entry.name)
        .collect();
    assert_eq!(names, ["new.txt"]);
}

#[tokio::test]
async fn a_first_write_that_ends_past_the_limit_leaves_the_file_clean_and_an_empty_write_fetches_nothing(
) {
    let (provider, vfs, _dir) = spooled_mount(Options::default(), 4, never_local);
    provider.add_file(&provider.root(), "a.txt", b"wxyz");
    provider.add_file(&provider.root(), "b.txt", b"abcd");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let other = vfs.lookup(Vfs::ROOT, "b.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    assert_eq!(vfs.write(handle, 4, b"q").await, Err(VfsError::NoSpace));
    let calls = provider.calls();
    let read = vfs.open(file.node, Access::Read).await.unwrap();
    assert_eq!(vfs.read(read, 0, 4).await.unwrap(), b"wxyz");
    assert!(provider.calls() > calls);
    vfs.write(handle, 0, b"Q").await.unwrap();
    assert_eq!(vfs.read(read, 0, 4).await.unwrap(), b"Qxyz");
    let calls = provider.calls();
    let other_handle = vfs.open(other.node, Access::Write).await.unwrap();
    vfs.write(other_handle, 100, b"").await.unwrap();
    assert_eq!(provider.calls(), calls);
    let other_read = vfs.open(other.node, Access::Read).await.unwrap();
    assert_eq!(vfs.read(other_read, 0, 4).await.unwrap(), b"abcd");
    assert!(provider.calls() > calls);
}

#[tokio::test(start_paused = true)]
async fn two_edits_of_one_spool_file_count_its_room_once() {
    let (_provider, vfs, _dir) = spooled_mount(Options::default(), 100, never_local);
    let (made, handle) = vfs.create(Vfs::ROOT, "a.txt", Access::Write).await.unwrap();
    vfs.write(handle, 0, b"aaaaaaaaaa").await.unwrap();
    let vfs_for_write = Arc::clone(&vfs);
    let writing = tokio::spawn(async move { vfs_for_write.write(handle, 10, &[b'b'; 40]).await });
    let vfs_for_truncate = Arc::clone(&vfs);
    let cutting = tokio::spawn(async move { vfs_for_truncate.truncate(made.node, 70).await });
    writing.await.unwrap().unwrap();
    cutting.await.unwrap().unwrap();
    let attrs = vfs.attributes(made.node).await.unwrap();
    let bytes = vfs.read(handle, 0, 200).await.unwrap();
    assert_eq!(attrs.size, bytes.len() as u64);
    let rest = (100 - attrs.size) as usize;
    vfs.write(handle, attrs.size, vec![b'c'; rest])
        .await
        .unwrap();
    assert_eq!(vfs.attributes(made.node).await.unwrap().size, 100);
}

#[tokio::test]
async fn a_new_file_renamed_over_a_new_file_leaves_one_name() {
    let (_provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let (first, first_handle) = vfs.create(Vfs::ROOT, "a.txt", Access::Write).await.unwrap();
    vfs.write(first_handle, 0, b"first").await.unwrap();
    let (second, second_handle) = vfs.create(Vfs::ROOT, "b.txt", Access::Write).await.unwrap();
    vfs.write(second_handle, 0, b"second").await.unwrap();
    assert_eq!(
        vfs.rename(Vfs::ROOT, "b.txt", Vfs::ROOT, "a.txt", IfExists::Refuse)
            .await,
        Err(VfsError::Conflict)
    );
    vfs.rename(Vfs::ROOT, "b.txt", Vfs::ROOT, "a.txt", IfExists::Replace)
        .await
        .unwrap();
    let found = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    assert_eq!(found.node, second.node);
    assert_eq!(
        vfs.lookup(Vfs::ROOT, "b.txt").await,
        Err(VfsError::NotFound)
    );
    assert_eq!(vfs.read(first_handle, 0, 10).await.unwrap(), b"first");
    vfs.release(first_handle);
    vfs.forget(first.node, 1);
    assert_eq!(spool_files(&dir), 1);
    assert_eq!(
        vfs.create_folder(Vfs::ROOT, "a.txt").await,
        Err(VfsError::Conflict)
    );
}

#[tokio::test]
async fn a_removed_spooled_item_deletes_its_spool_file_when_released() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    provider.add_file(&provider.root(), "a.txt", b"hello");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    vfs.write(handle, 0, b"H").await.unwrap();
    vfs.remove(Vfs::ROOT, "a.txt", Kind::File).await.unwrap();
    assert_eq!(vfs.read(handle, 0, 10).await.unwrap(), b"Hello");
    assert_eq!(spool_files(&dir), 1);
    vfs.release(handle);
    vfs.forget(file.node, 1);
    assert_eq!(spool_files(&dir), 0);
}

#[tokio::test]
async fn a_first_write_to_a_file_larger_than_the_limit_downloads_nothing() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 4, never_local);
    provider.add_file(&provider.root(), "big.bin", b"0123456789");
    let file = vfs.lookup(Vfs::ROOT, "big.bin").await.unwrap();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    let calls = provider.calls();
    assert_eq!(vfs.write(handle, 0, b"x").await, Err(VfsError::NoSpace));
    assert_eq!(provider.calls(), calls);
    provider.add_file(&provider.root(), "small.bin", b"0123");
    vfs.refresh();
    let small = vfs.lookup(Vfs::ROOT, "small.bin").await.unwrap();
    let handle = vfs.open(small.node, Access::Write).await.unwrap();
    let calls = provider.calls();
    assert_eq!(vfs.write(handle, 4, b"x").await, Err(VfsError::NoSpace));
    assert_eq!(provider.calls(), calls);
    assert_eq!(spool_files(&dir), 0);
}

#[tokio::test]
async fn new_files_follow_a_folder_whose_id_changes() {
    let (provider, vfs, _dir) = spooled_path_mount(Options::default(), 1024, never_local);
    provider.add_folder(&provider.root(), "docs");
    let docs = vfs.lookup(Vfs::ROOT, "docs").await.unwrap();
    let (made, handle) = vfs.create(docs.node, "a.txt", Access::Write).await.unwrap();
    vfs.write(handle, 0, b"hello").await.unwrap();
    vfs.rename(Vfs::ROOT, "docs", Vfs::ROOT, "papers", IfExists::Replace)
        .await
        .unwrap();
    let papers = vfs.lookup(Vfs::ROOT, "papers").await.unwrap();
    assert_eq!(papers.node, docs.node);
    let found = vfs.lookup(papers.node, "a.txt").await.unwrap();
    assert_eq!(found.node, made.node);
    assert_eq!(found.attributes.size, 5);
}

#[tokio::test]
async fn the_root_opened_for_write_is_a_folder_with_no_provider_call() {
    let (provider, vfs, _dir) = spooled_mount(Options::default(), 1024, never_local);
    assert_eq!(
        vfs.open(Vfs::ROOT, Access::Write).await,
        Err(VfsError::IsAFolder)
    );
    assert_eq!(vfs.truncate(Vfs::ROOT, 0).await, Err(VfsError::IsAFolder));
    assert_eq!(provider.calls(), 0);
}

#[tokio::test]
async fn a_spooled_file_renamed_over_another_keeps_its_bytes_when_ids_are_paths() {
    let (provider, vfs, _dir) = spooled_path_mount(Options::default(), 1024, never_local);
    for (draft, done, look_first) in [("a.txt", "b.txt", true), ("c.txt", "d.txt", false)] {
        provider.add_file(&provider.root(), draft, b"old");
        provider.add_file(&provider.root(), done, b"remote");
        vfs.refresh();
        let edited = vfs.lookup(Vfs::ROOT, draft).await.unwrap();
        if look_first {
            vfs.lookup(Vfs::ROOT, done).await.unwrap();
        }
        let handle = vfs.open(edited.node, Access::Write).await.unwrap();
        vfs.write(handle, 0, b"NEW").await.unwrap();
        vfs.rename(Vfs::ROOT, draft, Vfs::ROOT, done, IfExists::Replace)
            .await
            .unwrap();
        let found = vfs.lookup(Vfs::ROOT, done).await.unwrap();
        assert_eq!(found.node, edited.node);
        let read = vfs.open(found.node, Access::Read).await.unwrap();
        assert_eq!(vfs.read(read, 0, 10).await.unwrap(), b"NEW");
    }
}

#[tokio::test]
async fn truncating_a_clean_file_keeps_the_bytes_before_the_cut() {
    let (provider, vfs, _dir) = spooled_mount(Options::default(), 1024, never_local);
    let id = provider.add_file(&provider.root(), "a.txt", b"hello");
    provider.add_file(&provider.root(), "b.txt", b"hello");
    let cut = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let grown = vfs.lookup(Vfs::ROOT, "b.txt").await.unwrap();
    assert_eq!(vfs.truncate(cut.node, 2).await.unwrap().size, 2);
    assert_eq!(vfs.truncate(grown.node, 8).await.unwrap().size, 8);
    let read = vfs.open(cut.node, Access::Read).await.unwrap();
    assert_eq!(vfs.read(read, 0, 10).await.unwrap(), b"he");
    let read = vfs.open(grown.node, Access::Read).await.unwrap();
    assert_eq!(vfs.read(read, 0, 10).await.unwrap(), b"hello\0\0\0");
    assert_eq!(provider.contents(&id), b"hello");
}

#[tokio::test(start_paused = true)]
async fn two_first_writes_at_once_fetch_the_file_once() {
    let (provider, vfs, _dir) = spooled_mount(Options::default(), 1024, never_local);
    provider.add_file(&provider.root(), "a.txt", b"0123456789");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    provider.set_delay(Duration::from_secs(1));
    let calls = provider.calls();
    let first = tokio::spawn({
        let vfs = Arc::clone(&vfs);
        async move { vfs.write(handle, 0, b"A").await }
    });
    let second = tokio::spawn({
        let vfs = Arc::clone(&vfs);
        async move { vfs.write(handle, 9, b"Z").await }
    });
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    assert_eq!(provider.calls(), calls + 1);
    assert_eq!(vfs.read(handle, 0, 10).await.unwrap(), b"A12345678Z");
}

#[tokio::test(start_paused = true)]
async fn a_cancelled_first_write_leaves_the_file_clean() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    provider.add_file(&provider.root(), "a.txt", b"hello");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    provider.set_delay(Duration::from_secs(1));
    let writing = tokio::spawn({
        let vfs = Arc::clone(&vfs);
        async move { vfs.write(handle, 0, b"J").await }
    });
    while provider.in_flight() == 0 {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    writing.abort();
    assert!(writing.await.unwrap_err().is_cancelled());
    assert_eq!(spool_files(&dir), 0);
    provider.set_delay(Duration::ZERO);
    assert_eq!(vfs.read(handle, 0, 5).await.unwrap(), b"hello");
    vfs.write(handle, 0, b"J").await.unwrap();
    assert_eq!(vfs.read(handle, 0, 5).await.unwrap(), b"Jello");
}

/// An adapter runs each core call in a spawned task, so a call's future must be `Send`.
fn spawnable<F: std::future::Future + Send>(call: F) -> F {
    call
}

fn start_uploads(vfs: &Arc<Vfs>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(vfs.clone().run_uploads())
}

/// Ends the mount the way a crash does: `Drop for Vfs` never runs, so the spool folder stays as it was. A test that runs uploads aborts them first.
fn crash(vfs: Arc<Vfs>) {
    std::mem::forget(vfs);
}

#[tokio::test(start_paused = true)]
async fn a_new_file_uploads_after_the_settle_time() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    let (made, handle) = vfs.create(Vfs::ROOT, "a.txt", Access::Write).await.unwrap();
    vfs.write(handle, 0, b"hello").await.unwrap();
    vfs.release(handle);
    advance(settle() - Duration::from_millis(1)).await;
    assert_eq!(provider.uploads(), 0);
    assert!(root_items(&provider)
        .await
        .iter()
        .all(|item| item.name != "a.txt"));
    advance(Duration::from_millis(1)).await;
    yield_until("the new file did not upload", || {
        spool_files(&dir) == 0 && provider.uploads() >= 1
    })
    .await;
    let items = root_items(&provider).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "a.txt");
    assert_eq!(provider.contents(&items[0].id), b"hello");
    let found = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    assert_eq!(found.node, made.node);
}

/// Replays Finder's calls for a copy of two items (#629).
async fn finders_placeholders_never_reach(
    provider: Arc<MemoryProvider>,
    vfs: Arc<Vfs>,
    dir: tempfile::TempDir,
) {
    let _uploads = start_uploads(&vfs);
    let (big_placeholder, big) = vfs
        .create(Vfs::ROOT, "big.bin", Access::Write)
        .await
        .unwrap();
    vfs.release(big);
    let (small_placeholder, small) = vfs
        .create(Vfs::ROOT, "small.txt", Access::Write)
        .await
        .unwrap();
    vfs.release(small);

    vfs.remove(Vfs::ROOT, "big.bin", Kind::File).await.unwrap();
    // `create` counted a lookup. A removed new file keeps its spool file until that lookup is given back.
    vfs.forget(big_placeholder.node, 1);
    let (_, big) = vfs
        .create(Vfs::ROOT, "big.bin", Access::Write)
        .await
        .unwrap();
    vfs.write(big, 0, b"big bytes").await.unwrap();

    advance(settle() * 3).await;
    assert_eq!(provider.uploads(), 0);
    let unsent = vfs.unsent();
    assert_eq!(unsent.len(), 2);
    assert!(unsent
        .iter()
        .all(|unsent| unsent.state == UnsentState::Open));

    vfs.release(big);
    advance(settle() / 2).await;
    assert_eq!(provider.uploads(), 0);

    vfs.remove(Vfs::ROOT, "small.txt", Kind::File)
        .await
        .unwrap();
    vfs.forget(small_placeholder.node, 1);
    let (_, small) = vfs
        .create(Vfs::ROOT, "small.txt", Access::Write)
        .await
        .unwrap();
    vfs.write(small, 0, b"small").await.unwrap();
    vfs.release(small);

    advance(settle()).await;
    yield_until("the files did not upload", || {
        provider.uploads() == 2 && spool_files(&dir) == 0
    })
    .await;
    let mut got: Vec<(String, Vec<u8>)> = root_items(&provider)
        .await
        .into_iter()
        .map(|item| {
            let bytes = provider.contents(&item.id);
            (item.name, bytes)
        })
        .collect();
    got.sort();
    assert_eq!(
        got,
        vec![
            ("big.bin".to_string(), b"big bytes".to_vec()),
            ("small.txt".to_string(), b"small".to_vec()),
        ]
    );
    assert_eq!(provider.uploads(), 2);
    assert!(vfs.unsent().is_empty());
}

#[tokio::test(start_paused = true)]
async fn finders_placeholders_never_reach_the_remote() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    finders_placeholders_never_reach(provider, vfs, dir).await;
}

#[tokio::test(start_paused = true)]
async fn finders_placeholders_never_reach_a_remote_whose_ids_are_paths() {
    let (provider, vfs, dir) = spooled_path_mount(Options::default(), 1024, never_local);
    finders_placeholders_never_reach(provider, vfs, dir).await;
}

#[tokio::test(start_paused = true)]
async fn an_empty_new_file_waits_for_every_writer_in_the_mount() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    let (_, alone) = vfs
        .create(Vfs::ROOT, "alone.txt", Access::Write)
        .await
        .unwrap();
    vfs.release(alone);
    advance(settle()).await;
    yield_until("the empty file did not upload", || provider.uploads() == 1).await;
    let items = root_items(&provider).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "alone.txt");
    assert_eq!(provider.contents(&items[0].id), b"");

    let (_, open) = vfs
        .create(Vfs::ROOT, "open.txt", Access::Write)
        .await
        .unwrap();
    vfs.write(open, 0, b"x").await.unwrap();
    let (_, empty) = vfs
        .create(Vfs::ROOT, "empty.txt", Access::Write)
        .await
        .unwrap();
    vfs.release(empty);
    let (_, full) = vfs
        .create(Vfs::ROOT, "full.txt", Access::Write)
        .await
        .unwrap();
    vfs.write(full, 0, b"y").await.unwrap();
    vfs.release(full);

    advance(settle()).await;
    yield_until("the non-empty file did not upload", || {
        provider.uploads() == 2
    })
    .await;
    advance(settle() * 2).await;
    assert_eq!(provider.uploads(), 2);
    let empty = vfs
        .unsent()
        .into_iter()
        .find(|unsent| unsent.name == "empty.txt")
        .unwrap();
    assert_eq!(empty.state, UnsentState::Open);

    vfs.release(open);
    advance(settle() - Duration::from_millis(1)).await;
    assert_eq!(provider.uploads(), 2);
    advance(Duration::from_millis(1)).await;
    yield_until("the held files did not upload", || {
        provider.uploads() == 4 && spool_files(&dir) == 0
    })
    .await;
    let mut got: Vec<(String, Vec<u8>)> = root_items(&provider)
        .await
        .into_iter()
        .map(|item| {
            let bytes = provider.contents(&item.id);
            (item.name, bytes)
        })
        .collect();
    got.sort();
    assert_eq!(
        got,
        vec![
            ("alone.txt".to_string(), b"".to_vec()),
            ("empty.txt".to_string(), b"".to_vec()),
            ("full.txt".to_string(), b"y".to_vec()),
            ("open.txt".to_string(), b"x".to_vec()),
        ]
    );
    assert!(vfs.unsent().is_empty());
}

#[tokio::test(start_paused = true)]
async fn an_empty_new_file_keeps_its_retry_wait_when_another_writer_closes() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    provider.add_file(&provider.root(), "other.txt", b"old");
    let _uploads = start_uploads(&vfs);
    // `create` asks the provider for the folder until the mount holds its listing and attributes. Those calls are not the upload.
    vfs.attributes(Vfs::ROOT).await.unwrap();
    vfs.lookup(Vfs::ROOT, "other.txt").await.unwrap();
    provider.fail_with(Some(VfsError::Offline));
    let (_, empty) = vfs
        .create(Vfs::ROOT, "empty.txt", Access::Write)
        .await
        .unwrap();
    vfs.release(empty);
    advance(settle()).await;
    yield_until("the failure was not recorded", || {
        vfs.unsent().iter().any(|unsent| {
            unsent.name == "empty.txt" && unsent.state == UnsentState::Failed(VfsError::Offline)
        }) && provider.uploads() == 1
    })
    .await;
    provider.fail_with(None);
    // The retry is due 5 seconds after that failure.
    advance(Duration::from_secs(1)).await;
    let other = vfs.lookup(Vfs::ROOT, "other.txt").await.unwrap();
    let handle = vfs.open(other.node, Access::Write).await.unwrap();
    assert!(vfs.unsent().iter().any(|unsent| {
        unsent.name == "empty.txt" && unsent.state == UnsentState::Failed(VfsError::Offline)
    }));
    advance(Duration::from_millis(500)).await;
    vfs.release(handle);
    advance(Duration::from_millis(3_499)).await;
    assert_eq!(provider.uploads(), 1);
    advance(Duration::from_millis(1)).await;
    yield_until("the retry did not upload", || {
        provider.uploads() == 2 && spool_files(&dir) == 0
    })
    .await;
    let items = root_items(&provider).await;
    let empty = items.iter().find(|item| item.name == "empty.txt").unwrap();
    assert_eq!(provider.contents(&empty.id), b"");
}

#[tokio::test(start_paused = true)]
async fn an_empty_new_file_whose_retry_comes_while_a_writer_is_open_waits_for_it() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    provider.add_file(&provider.root(), "other.txt", b"old");
    let _uploads = start_uploads(&vfs);
    // `create` asks the provider for the folder until the mount holds its listing and attributes. Those calls are not the upload.
    vfs.attributes(Vfs::ROOT).await.unwrap();
    let other = vfs.lookup(Vfs::ROOT, "other.txt").await.unwrap();
    provider.fail_with(Some(VfsError::Offline));
    let (_, empty) = vfs
        .create(Vfs::ROOT, "empty.txt", Access::Write)
        .await
        .unwrap();
    vfs.release(empty);
    advance(settle()).await;
    yield_until("the first try did not fail", || provider.uploads() == 1).await;
    provider.fail_with(None);
    // The retry is due 5 seconds after that failure. The writer is still open then.
    advance(Duration::from_secs(1)).await;
    let handle = vfs.open(other.node, Access::Write).await.unwrap();
    advance(Duration::from_secs(5)).await;
    assert_eq!(provider.uploads(), 1);
    assert!(vfs.unsent().iter().any(|unsent| {
        unsent.name == "empty.txt" && unsent.state == UnsentState::Failed(VfsError::Offline)
    }));
    vfs.release(handle);
    advance(settle() - Duration::from_millis(1)).await;
    assert_eq!(provider.uploads(), 1);
    advance(Duration::from_millis(1)).await;
    yield_until("the retry did not upload", || {
        provider.uploads() == 2 && spool_files(&dir) == 0
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn a_refused_empty_new_file_shows_its_refusal_while_another_file_is_written() {
    let (provider, vfs, _dir) = spooled_mount(Options::default(), 1024, never_local);
    provider.add_file(&provider.root(), "other.txt", b"old");
    let _uploads = start_uploads(&vfs);
    // `create` asks the provider for the folder until the mount holds its listing and attributes. Those calls are not the upload.
    vfs.attributes(Vfs::ROOT).await.unwrap();
    vfs.lookup(Vfs::ROOT, "other.txt").await.unwrap();
    provider.fail_with(Some(VfsError::AccessDenied));
    let (_, empty) = vfs
        .create(Vfs::ROOT, "empty.txt", Access::Write)
        .await
        .unwrap();
    vfs.release(empty);
    advance(settle()).await;
    yield_until("the refusal was not recorded", || {
        vfs.unsent().iter().any(|unsent| {
            unsent.name == "empty.txt"
                && unsent.state == UnsentState::Refused(VfsError::AccessDenied)
        })
    })
    .await;
    provider.fail_with(None);
    let other = vfs.lookup(Vfs::ROOT, "other.txt").await.unwrap();
    let handle = vfs.open(other.node, Access::Write).await.unwrap();
    assert!(vfs.unsent().iter().any(|unsent| {
        unsent.name == "empty.txt" && unsent.state == UnsentState::Refused(VfsError::AccessDenied)
    }));
    vfs.release(handle);
    advance(settle() * 2).await;
    assert_eq!(provider.uploads(), 1);
}

#[tokio::test(start_paused = true)]
async fn an_edited_file_keeps_its_id_and_shows_the_new_revision() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    let id = provider.add_file(&provider.root(), "a.txt", b"old");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let old_revision = file.attributes.revision.clone();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    vfs.write(handle, 0, b"new").await.unwrap();
    vfs.release(handle);
    advance(settle() - Duration::from_millis(1)).await;
    assert_eq!(provider.uploads(), 0);
    assert_eq!(provider.contents(&id), b"old");
    advance(Duration::from_millis(1)).await;
    yield_until("the edit did not upload", || {
        spool_files(&dir) == 0 && provider.uploads() == 1
    })
    .await;
    assert_eq!(provider.contents(&id), b"new");
    let items = root_items(&provider).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, id);
    assert_ne!(items[0].attributes.revision, old_revision);
    let attrs = vfs.attributes(file.node).await.unwrap();
    assert_eq!(attrs.size, 3);
    assert_eq!(attrs.revision, items[0].attributes.revision);
}

#[tokio::test(start_paused = true)]
async fn a_file_reopened_inside_the_settle_time_uploads_once() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    let id = provider.add_file(&provider.root(), "a.txt", b"old");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    vfs.write(handle, 0, b"one").await.unwrap();
    vfs.release(handle);
    advance(Duration::from_secs(1)).await;
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    vfs.write(handle, 0, b"two!").await.unwrap();
    vfs.release(handle);
    advance(settle() - Duration::from_millis(1)).await;
    assert_eq!(provider.uploads(), 0);
    assert_eq!(provider.contents(&id), b"old");
    advance(Duration::from_millis(1)).await;
    yield_until("the later bytes did not upload", || {
        spool_files(&dir) == 0 && provider.uploads() == 1
    })
    .await;
    assert_eq!(provider.contents(&id), b"two!");
}

#[tokio::test(start_paused = true)]
async fn a_remote_change_before_the_upload_keeps_a_conflict_copy() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    let id = provider.add_file(&provider.root(), "a.txt", b"remote");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    vfs.write(handle, 0, b"local!").await.unwrap();
    vfs.release(handle);
    provider.write(&id, b"changed");
    advance(settle()).await;
    // The copy's create and the item call after it both end before the spool file goes.
    yield_until("the conflict copy did not land", || {
        spool_files(&dir) == 0 && provider.in_flight() == 0
    })
    .await;
    assert_eq!(provider.contents(&id), b"changed");
    let items = root_items(&provider).await;
    let copies: Vec<_> = items
        .iter()
        .filter(|item| item.name.starts_with("a (conflict ") && item.name.ends_with(").txt"))
        .collect();
    assert_eq!(copies.len(), 1);
    assert_eq!(provider.contents(&copies[0].id), b"local!");
    let read = vfs.open(file.node, Access::Read).await.unwrap();
    assert_eq!(vfs.read(read, 0, 10).await.unwrap(), b"changed");
    assert_eq!(vfs.attributes(file.node).await.unwrap().size, 7);
    // The mount lists the copy from the listing it holds, with no new provider call.
    let calls = provider.calls();
    vfs.lookup(Vfs::ROOT, &copies[0].name).await.unwrap();
    assert_eq!(provider.calls(), calls);
}

#[tokio::test(start_paused = true)]
async fn a_remote_remove_before_the_upload_creates_the_file_again() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    let id = provider.add_file(&provider.root(), "a.txt", b"remote");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    vfs.write(handle, 0, b"local!").await.unwrap();
    vfs.release(handle);
    provider.remote_remove(&id);
    advance(settle()).await;
    yield_until("the file was not created again", || spool_files(&dir) == 0).await;
    let items = root_items(&provider).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "a.txt");
    assert_ne!(items[0].id, id);
    assert_eq!(provider.contents(&items[0].id), b"local!");
    let found = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    assert_eq!(found.node, file.node);
}

#[tokio::test(start_paused = true)]
async fn a_failed_upload_retries_after_five_seconds_then_ten() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    let (made, handle) = vfs.create(Vfs::ROOT, "a.txt", Access::Write).await.unwrap();
    vfs.write(handle, 0, b"hello").await.unwrap();
    vfs.release(handle);
    provider.fail_with(Some(VfsError::Offline));
    advance(settle()).await;
    yield_until("the first failure was not recorded", || {
        matches!(
            vfs.unsent().first().map(|unsent| unsent.state.clone()),
            Some(UnsentState::Failed(VfsError::Offline))
        )
    })
    .await;
    let read = vfs.open(made.node, Access::Read).await.unwrap();
    assert_eq!(vfs.read(read, 0, 10).await.unwrap(), b"hello");
    assert_eq!(provider.uploads(), 1);
    advance(Duration::from_secs(5) - Duration::from_millis(1)).await;
    assert_eq!(provider.uploads(), 1);
    advance(Duration::from_millis(1)).await;
    yield_until("the second try did not start", || provider.uploads() == 2).await;
    yield_until("the second failure was not recorded", || {
        matches!(
            vfs.unsent().first().map(|unsent| unsent.state.clone()),
            Some(UnsentState::Failed(VfsError::Offline))
        )
    })
    .await;
    advance(Duration::from_secs(9)).await;
    assert_eq!(provider.uploads(), 2);
    assert!(matches!(
        vfs.unsent().first().map(|unsent| unsent.state.clone()),
        Some(UnsentState::Failed(VfsError::Offline))
    ));
    provider.fail_with(None);
    let calls = provider.calls();
    advance(Duration::from_secs(1)).await;
    yield_until("the retry did not upload", || {
        spool_files(&dir) == 0 && provider.uploads() == 3
    })
    .await;
    assert_eq!(provider.calls(), calls + 1);
    let items = root_items(&provider).await;
    assert_eq!(items.len(), 1);
    assert_eq!(provider.contents(&items[0].id), b"hello");
}

#[tokio::test(start_paused = true)]
async fn an_upload_does_not_take_a_provider_call_slot() {
    let options = Options {
        max_calls: 1,
        ..Options::default()
    };
    let (provider, vfs, _dir) = spooled_mount(options, 1024, never_local);
    let _uploads = start_uploads(&vfs);
    provider.add_file(&provider.root(), "b.txt", b"other");
    provider.set_upload_delay(Duration::from_secs(60 * 60));
    let (_made, handle) = vfs.create(Vfs::ROOT, "a.txt", Access::Write).await.unwrap();
    vfs.write(handle, 0, b"hello").await.unwrap();
    vfs.release(handle);
    advance(settle()).await;
    yield_until("the upload did not start", || {
        provider.uploads() == 1 && provider.in_flight() >= 1
    })
    .await;
    vfs.refresh();
    let found = vfs.lookup(Vfs::ROOT, "b.txt").await.unwrap();
    assert_eq!(found.name, "b.txt");
    assert_eq!(provider.uploads(), 1);
    assert!(provider.in_flight() >= 1);
}

#[tokio::test(start_paused = true)]
async fn a_write_while_uploading_sends_the_later_bytes() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    provider.set_upload_delay(Duration::from_secs(30));
    let id = provider.add_file(&provider.root(), "a.txt", b"old");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    vfs.write(handle, 0, b"one").await.unwrap();
    vfs.release(handle);
    advance(settle()).await;
    yield_until("the first upload did not start", || {
        provider.uploads() == 1 && provider.in_flight() >= 1
    })
    .await;
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    vfs.write(handle, 0, b"two!").await.unwrap();
    yield_until("the stopped upload is still in flight", || {
        provider.in_flight() == 0
    })
    .await;
    assert_eq!(provider.contents(&id), b"old");
    assert_eq!(root_items(&provider).await.len(), 1);
    vfs.release(handle);
    advance(settle()).await;
    yield_until("the second upload did not start", || {
        provider.uploads() == 2
    })
    .await;
    advance(Duration::from_secs(30)).await;
    yield_until("the later bytes did not land", || spool_files(&dir) == 0).await;
    assert_eq!(provider.uploads(), 2);
    assert_eq!(provider.contents(&id), b"two!");
    assert_eq!(root_items(&provider).await.len(), 1);
}

#[tokio::test(start_paused = true)]
async fn an_upload_that_does_not_move_times_out_and_retries() {
    let options = Options {
        upload_stall: Duration::from_secs(5),
        ..Options::default()
    };
    let (provider, vfs, _dir) = spooled_mount(options, 1024, never_local);
    let _uploads = start_uploads(&vfs);
    provider.set_upload_delay(Duration::from_secs(60 * 60));
    let (_made, handle) = vfs.create(Vfs::ROOT, "a.txt", Access::Write).await.unwrap();
    vfs.write(handle, 0, b"hello").await.unwrap();
    vfs.release(handle);
    advance(settle()).await;
    yield_until("the stalled upload did not start", || {
        provider.uploads() == 1
    })
    .await;
    advance(Duration::from_secs(5)).await;
    yield_until("the stall was not recorded", || {
        matches!(
            vfs.unsent().first().map(|unsent| unsent.state.clone()),
            Some(UnsentState::Failed(VfsError::TimedOut))
        )
    })
    .await;
    advance(Duration::from_secs(5) - Duration::from_millis(1)).await;
    assert_eq!(provider.uploads(), 1);
    advance(Duration::from_millis(1)).await;
    yield_until("the stalled upload was not retried", || {
        provider.uploads() == 2
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn unsent_shows_open_waiting_and_uploading() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    provider.set_upload_delay(Duration::from_secs(10));
    let (_made, handle) = vfs.create(Vfs::ROOT, "a.txt", Access::Write).await.unwrap();
    assert_eq!(vfs.unsent()[0].state, UnsentState::Open);
    vfs.write(handle, 0, b"hello").await.unwrap();
    let unsent = &vfs.unsent()[0];
    assert_eq!(unsent.state, UnsentState::Open);
    assert_eq!(
        (&unsent.folder, unsent.name.as_str(), unsent.size),
        (&provider.root(), "a.txt", 5)
    );
    vfs.release(handle);
    assert_eq!(vfs.unsent()[0].state, UnsentState::Waiting);
    advance(settle()).await;
    yield_until("the file did not show as uploading", || {
        matches!(
            vfs.unsent().first().map(|unsent| unsent.state.clone()),
            Some(UnsentState::Uploading { sent: 0 })
        )
    })
    .await;
    advance(Duration::from_secs(10)).await;
    yield_until("the uploaded file is still unsent", || {
        vfs.unsent().is_empty() && spool_files(&dir) == 0
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn retry_now_sends_a_failed_file_at_once() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    let (_failed, handle) = vfs.create(Vfs::ROOT, "b.txt", Access::Write).await.unwrap();
    vfs.write(handle, 0, b"later").await.unwrap();
    vfs.release(handle);
    provider.fail_with(Some(VfsError::Offline));
    advance(settle()).await;
    yield_until("the failed file is not unsent", || {
        matches!(
            vfs.unsent().first().map(|unsent| unsent.state.clone()),
            Some(UnsentState::Failed(VfsError::Offline))
        )
    })
    .await;
    let failed = vfs.unsent()[0].node;
    provider.fail_with(None);
    let calls = provider.calls();
    let uploads = provider.uploads();
    vfs.retry_now(failed).unwrap();
    yield_until("retry_now did not upload", || {
        provider.uploads() == uploads + 1 && spool_files(&dir) == 0
    })
    .await;
    assert_eq!(provider.calls(), calls + 1);
    assert!(vfs.unsent().is_empty());
}

#[tokio::test(start_paused = true)]
async fn discard_unsent_gives_an_item_back_its_remote_bytes() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    provider.set_upload_delay(Duration::from_secs(60 * 60));
    let id = provider.add_file(&provider.root(), "c.txt", b"remote");
    let file = vfs.lookup(Vfs::ROOT, "c.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    vfs.write(handle, 0, b"LOCAL!").await.unwrap();
    vfs.release(handle);
    advance(settle()).await;
    yield_until("the item upload did not start", || {
        provider.in_flight() >= 1
    })
    .await;
    spawnable(vfs.discard_unsent(file.node)).await.unwrap();
    yield_until("the discarded upload is still in flight", || {
        provider.in_flight() == 0
    })
    .await;
    assert_eq!(spool_files(&dir), 0);
    let read = vfs.open(file.node, Access::Read).await.unwrap();
    assert_eq!(vfs.read(read, 0, 10).await.unwrap(), b"remote");
    assert_eq!(provider.contents(&id), b"remote");
    assert_eq!(provider.uploads(), 1);
}

#[tokio::test(start_paused = true)]
async fn discard_unsent_unnames_a_new_file_and_keeps_its_open_handle() {
    let (provider, vfs, _dir) = spooled_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    let (made, handle) = vfs.create(Vfs::ROOT, "d.txt", Access::Write).await.unwrap();
    vfs.write(handle, 0, b"keep").await.unwrap();
    vfs.discard_unsent(made.node).await.unwrap();
    assert_eq!(
        vfs.lookup(Vfs::ROOT, "d.txt").await,
        Err(VfsError::NotFound)
    );
    assert_eq!(vfs.read(handle, 0, 10).await.unwrap(), b"keep");
    vfs.release(handle);
    advance(settle() + Duration::from_secs(1)).await;
    assert!(root_items(&provider).await.is_empty());
    assert_eq!(provider.uploads(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_new_file_removed_while_it_uploads_leaves_nothing() {
    let (provider, vfs, _dir) = spooled_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    provider.set_upload_delay(Duration::from_secs(30));
    let (_made, handle) = vfs.create(Vfs::ROOT, "a.txt", Access::Write).await.unwrap();
    vfs.write(handle, 0, b"hello").await.unwrap();
    vfs.release(handle);
    advance(settle()).await;
    yield_until("the create did not start", || {
        provider.uploads() == 1 && provider.in_flight() >= 1
    })
    .await;
    vfs.remove(Vfs::ROOT, "a.txt", Kind::File).await.unwrap();
    yield_until("the removed upload is still in flight", || {
        provider.in_flight() == 0
    })
    .await;
    advance(Duration::from_secs(30)).await;
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert!(root_items(&provider).await.is_empty());
    assert_eq!(provider.uploads(), 1);
}

#[tokio::test(start_paused = true)]
async fn set_modified_goes_out_with_a_spooled_file_and_leaves_a_clean_file() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    let (made, handle) = vfs.create(Vfs::ROOT, "a.txt", Access::Write).await.unwrap();
    vfs.write(handle, 0, b"hello").await.unwrap();
    let modified = UNIX_EPOCH + Duration::from_secs(50_000);
    let attrs = spawnable(vfs.set_modified(made.node, modified))
        .await
        .unwrap();
    assert_eq!(attrs.modified, modified);
    vfs.release(handle);
    advance(settle()).await;
    yield_until("the modified time did not upload", || {
        spool_files(&dir) == 0
    })
    .await;
    let items = root_items(&provider).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].attributes.modified, modified);

    provider.add_file(&provider.root(), "b.txt", b"clean");
    vfs.refresh();
    let clean = vfs.lookup(Vfs::ROOT, "b.txt").await.unwrap();
    let before = vfs.attributes(clean.node).await.unwrap();
    let calls = provider.calls();
    let uploads = provider.uploads();
    let after = spawnable(vfs.set_modified(clean.node, UNIX_EPOCH + Duration::from_secs(9)))
        .await
        .unwrap();
    assert_eq!(after, before);
    assert_eq!(provider.uploads(), uploads);
    assert_eq!(provider.calls(), calls);
    let remote = root_items(&provider)
        .await
        .into_iter()
        .find(|item| item.name == "b.txt")
        .unwrap();
    assert_eq!(remote.attributes.modified, before.modified);
}

#[tokio::test(start_paused = true)]
async fn a_new_file_renamed_over_an_item_uploads_as_a_replace() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    let id = provider.add_file(&provider.root(), "a.txt", b"old");
    let (made, handle) = vfs.create(Vfs::ROOT, "b.txt", Access::Write).await.unwrap();
    vfs.write(handle, 0, b"new").await.unwrap();
    vfs.rename(Vfs::ROOT, "b.txt", Vfs::ROOT, "a.txt", IfExists::Replace)
        .await
        .unwrap();
    vfs.release(handle);
    advance(settle()).await;
    yield_until("the replace did not upload", || {
        spool_files(&dir) == 0 && provider.uploads() == 1
    })
    .await;
    let items = root_items(&provider).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, id);
    assert_eq!(items[0].name, "a.txt");
    assert_eq!(provider.contents(&id), b"new");
    assert!(items.iter().all(|item| !item.name.contains("conflict")));
    let found = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    assert_eq!(found.node, made.node);
}

#[tokio::test(start_paused = true)]
async fn a_finder_file_never_uploads() {
    let (provider, vfs, _dir) = spooled_mount(Options::default(), 1024, finder_files);
    let _uploads = start_uploads(&vfs);
    let (_made, handle) = vfs
        .create(Vfs::ROOT, ".DS_Store", Access::Write)
        .await
        .unwrap();
    vfs.write(handle, 0, b"finder").await.unwrap();
    vfs.release(handle);
    advance(settle() + Duration::from_secs(1)).await;
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert!(root_items(&provider).await.is_empty());
    assert!(vfs.unsent().is_empty());
    assert_eq!(provider.uploads(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_stale_answer_bases_the_upload_on_the_bytes_it_copied() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    let id = provider.add_file(&provider.root(), "a.txt", b"v1!!");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    advance(Options::default().attributes_ttl).await;
    provider.write(&id, b"v2!!");
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    vfs.write(handle, 0, b"z").await.unwrap();
    vfs.release(handle);
    advance(settle()).await;
    yield_until("the fresh revision did not upload", || {
        spool_files(&dir) == 0
    })
    .await;
    // The copy holds the other client's bytes, and the upload is not taken for a conflict.
    assert_eq!(provider.contents(&id), b"z2!!");
    assert_eq!(provider.uploads(), 1);
    assert_eq!(root_items(&provider).await.len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_writer_held_past_a_retry_uploads_after_it_closes() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    let id = provider.add_file(&provider.root(), "a.txt", b"old");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    vfs.write(handle, 0, b"new").await.unwrap();
    vfs.release(handle);
    provider.fail_with(Some(VfsError::Offline));
    advance(settle()).await;
    yield_until("the failure was not recorded", || {
        provider.uploads() == 1
            && matches!(
                vfs.unsent().first().map(|unsent| unsent.state.clone()),
                Some(UnsentState::Failed(VfsError::Offline))
            )
    })
    .await;
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    advance(Duration::from_secs(6)).await;
    assert_eq!(provider.uploads(), 1);
    assert_eq!(provider.contents(&id), b"old");
    provider.fail_with(None);
    advance(Duration::from_secs(1)).await;
    assert_eq!(provider.uploads(), 1);
    vfs.release(handle);
    advance(settle() - Duration::from_millis(1)).await;
    assert_eq!(provider.uploads(), 1);
    advance(Duration::from_millis(1)).await;
    yield_until("the file did not upload after the writer closed", || {
        spool_files(&dir) == 0 && provider.uploads() == 2
    })
    .await;
    assert_eq!(provider.contents(&id), b"new");
}

#[tokio::test(start_paused = true)]
async fn a_listing_that_answers_after_the_confirm_keeps_the_new_size() {
    let options = Options {
        attributes_ttl: Duration::from_secs(60),
        ..Options::default()
    };
    let (provider, vfs, _dir) = spooled_mount(options, 1024, never_local);
    let _uploads = start_uploads(&vfs);
    provider.add_file(&provider.root(), "a.txt", b"old");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let old_revision = file.attributes.revision.clone();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    vfs.write(handle, 0, b"new!").await.unwrap();
    provider.set_upload_delay(Duration::from_secs(10));
    provider.set_delay(Duration::from_secs(10));
    vfs.release(handle);
    advance(settle()).await;
    yield_until("the upload did not start", || provider.uploads() == 1).await;
    advance(Duration::from_secs(5)).await;
    vfs.refresh();
    let vfs_lookup = Arc::clone(&vfs);
    let lookup = tokio::spawn(async move { vfs_lookup.lookup(Vfs::ROOT, "a.txt").await });
    let found = lookup.await.unwrap().unwrap();
    assert_eq!(found.attributes.size, 4);
    assert_ne!(found.attributes.revision, old_revision);
    assert!(found.attributes.revision.is_some());
    let attrs = vfs.attributes(file.node).await.unwrap();
    assert_eq!(attrs.size, 4);
    assert_eq!(attrs.revision, found.attributes.revision);
}

#[tokio::test(start_paused = true)]
async fn a_rename_that_changes_the_id_during_a_replace_keeps_one_item() {
    let (provider, vfs, dir) = spooled_path_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    provider.set_upload_delay(Duration::from_secs(5));
    provider.add_file(&provider.root(), "a.txt", b"old");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    vfs.write(handle, 0, b"new").await.unwrap();
    vfs.release(handle);
    advance(settle()).await;
    yield_until("the replace did not start", || {
        provider.uploads() == 1 && provider.in_flight() >= 1
    })
    .await;
    vfs.rename(Vfs::ROOT, "a.txt", Vfs::ROOT, "b.txt", IfExists::Replace)
        .await
        .unwrap();
    advance(Duration::from_secs(5)).await;
    yield_until("the replace was not tried again", || {
        provider.uploads() >= 2
    })
    .await;
    advance(Duration::from_secs(5)).await;
    yield_until("the renamed item did not get the new bytes", || {
        spool_files(&dir) == 0
    })
    .await;
    let items = root_items(&provider).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "b.txt");
    assert_eq!(items[0].id, path("/b.txt"));
    assert_eq!(provider.contents(&items[0].id), b"new");
    assert!(items.iter().all(|item| item.name != "a.txt"));
    assert_eq!(provider.uploads(), 2);
}

#[tokio::test(start_paused = true)]
async fn the_upload_limit_holds_a_third_file_until_one_ends() {
    let options = Options {
        max_uploads: 2,
        ..Options::default()
    };
    let (provider, vfs, dir) = spooled_mount(options, 1024, never_local);
    let _uploads = start_uploads(&vfs);
    provider.set_upload_delay(Duration::from_secs(10));
    for name in ["a.txt", "b.txt", "c.txt"] {
        let (_made, handle) = vfs.create(Vfs::ROOT, name, Access::Write).await.unwrap();
        vfs.write(handle, 0, name.as_bytes()).await.unwrap();
        vfs.release(handle);
    }
    advance(settle()).await;
    yield_until("two uploads did not start", || provider.uploads() == 2).await;
    advance(Duration::from_secs(5)).await;
    assert_eq!(provider.uploads(), 2);
    advance(Duration::from_secs(5)).await;
    yield_until("the third upload did not start", || provider.uploads() == 3).await;
    advance(Duration::from_secs(10)).await;
    yield_until("the third upload did not land", || spool_files(&dir) == 0).await;
    assert_eq!(root_items(&provider).await.len(), 3);
}

#[tokio::test(start_paused = true)]
async fn progress_keeps_an_upload_longer_than_the_stall_time_going() {
    let options = Options {
        upload_stall: Duration::from_secs(5),
        ..Options::default()
    };
    let (provider, vfs, dir) = spooled_mount(options, 4096, never_local);
    let _uploads = start_uploads(&vfs);
    provider.set_upload_delay(Duration::from_secs(8));
    let (_made, handle) = vfs.create(Vfs::ROOT, "a.txt", Access::Write).await.unwrap();
    vfs.write(handle, 0, &[7u8; 1000]).await.unwrap();
    vfs.release(handle);
    advance(settle()).await;
    yield_until("the upload did not start", || provider.uploads() == 1).await;
    advance(Duration::from_secs(4)).await;
    assert_eq!(vfs.unsent()[0].state, UnsentState::Uploading { sent: 500 });
    // Past the stall time counted from the start, the progress at 4 s keeps the try going.
    advance(Duration::from_secs(2)).await;
    assert_eq!(vfs.unsent()[0].state, UnsentState::Uploading { sent: 500 });
    assert_eq!(provider.uploads(), 1);
    advance(Duration::from_secs(2)).await;
    yield_until("the upload did not land", || spool_files(&dir) == 0).await;
    assert_eq!(provider.uploads(), 1);
    assert_eq!(root_items(&provider).await.len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_create_that_answers_after_its_file_was_removed_is_removed_again() {
    let (provider, vfs, _dir) = spooled_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    let (_made, handle) = vfs.create(Vfs::ROOT, "a.txt", Access::Write).await.unwrap();
    vfs.write(handle, 0, b"hello").await.unwrap();
    vfs.release(handle);
    provider.set_upload_delay(Duration::from_secs(5));
    provider.set_delay(Duration::from_secs(10));
    advance(settle()).await;
    yield_until("the create did not start", || provider.uploads() == 1).await;
    // The remove lists the folder for 10 seconds while it holds the changes lock, so the create's answer at 5 seconds waits behind it.
    vfs.refresh();
    let remover = Arc::clone(&vfs);
    let remove = tokio::spawn(async move { remover.remove(Vfs::ROOT, "a.txt", Kind::File).await });
    remove.await.unwrap().unwrap();
    // The stray's remove waits the 10-second delay too.
    advance(Duration::from_secs(10)).await;
    yield_until("the stray was not removed", || provider.in_flight() == 0).await;
    provider.set_delay(Duration::ZERO);
    assert!(root_items(&provider).await.is_empty());
    assert_eq!(provider.uploads(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_write_while_a_create_is_confirmed_replaces_on_the_new_revision() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    let (made, handle) = vfs.create(Vfs::ROOT, "a.txt", Access::Write).await.unwrap();
    vfs.write(handle, 0, b"one").await.unwrap();
    vfs.release(handle);
    provider.set_upload_delay(Duration::from_secs(5));
    provider.set_delay(Duration::from_secs(10));
    advance(settle()).await;
    yield_until("the create did not start", || provider.uploads() == 1).await;
    // The new folder holds the changes lock for 20 seconds, so the create's answer at 5 seconds waits behind it.
    vfs.refresh();
    let maker = Arc::clone(&vfs);
    let folder = tokio::spawn(async move { maker.create_folder(Vfs::ROOT, "hold").await });
    advance(Duration::from_secs(7)).await;
    // The create landed and its confirm waits behind the folder.
    provider.set_delay(Duration::ZERO);
    let landed = root_items(&provider).await;
    provider.set_delay(Duration::from_secs(10));
    assert_eq!(landed.len(), 1);
    assert_eq!(provider.contents(&landed[0].id), b"one");
    let handle = vfs.open(made.node, Access::Write).await.unwrap();
    vfs.write(handle, 0, b"two!").await.unwrap();
    vfs.release(handle);
    folder.await.unwrap().unwrap();
    provider.set_delay(Duration::ZERO);
    advance(settle() + Duration::from_secs(5)).await;
    yield_until("the later bytes did not land", || spool_files(&dir) == 0).await;
    let files: Vec<Item> = root_items(&provider)
        .await
        .into_iter()
        .filter(|item| item.attributes.kind == Kind::File)
        .collect();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].id, landed[0].id);
    assert_eq!(provider.contents(&files[0].id), b"two!");
    assert_eq!(provider.uploads(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_writer_closed_during_a_failing_try_does_not_cut_the_retry_wait() {
    let (provider, vfs, _dir) = spooled_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    let (made, handle) = vfs.create(Vfs::ROOT, "a.txt", Access::Write).await.unwrap();
    vfs.write(handle, 0, b"hello").await.unwrap();
    vfs.release(handle);
    provider.fail_with(Some(VfsError::Offline));
    advance(settle()).await;
    yield_until("the first try did not fail", || provider.uploads() == 1).await;
    provider.set_upload_delay(Duration::from_secs(1));
    advance(Duration::from_secs(5)).await;
    yield_until("the second try did not start", || provider.uploads() == 2).await;
    // An app opens the file for write and closes it without writing while the try runs.
    let handle = vfs.open(made.node, Access::Write).await.unwrap();
    vfs.release(handle);
    advance(Duration::from_secs(1)).await;
    advance(Duration::from_secs(10) - Duration::from_millis(1)).await;
    assert_eq!(provider.uploads(), 2);
    // A discard leaves nothing in the queue for the uploader to trip on.
    vfs.discard_unsent(made.node).await.unwrap();
    provider.fail_with(None);
    provider.set_upload_delay(Duration::ZERO);
    let (_other, handle) = vfs.create(Vfs::ROOT, "b.txt", Access::Write).await.unwrap();
    vfs.write(handle, 0, b"other").await.unwrap();
    vfs.release(handle);
    advance(settle()).await;
    // The discarded file's spool file stays while the OS holds a lookup on it, so the provider is what tells.
    yield_until("the uploader stopped", || provider.uploads() == 3).await;
    let items = root_items(&provider).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "b.txt");
}

#[tokio::test(start_paused = true)]
async fn a_truncate_with_no_open_handle_uploads_after_the_settle_time() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    let id = provider.add_file(&provider.root(), "a.txt", b"hello");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    vfs.truncate(file.node, 2).await.unwrap();
    advance(settle() - Duration::from_millis(1)).await;
    assert_eq!(provider.uploads(), 0);
    advance(Duration::from_millis(1)).await;
    yield_until("the cut file did not upload", || spool_files(&dir) == 0).await;
    assert_eq!(provider.contents(&id), b"he");
}

#[tokio::test(start_paused = true)]
async fn a_new_file_renamed_while_it_uploads_goes_up_under_the_new_name() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    provider.set_upload_delay(Duration::from_secs(10));
    let (_made, handle) = vfs.create(Vfs::ROOT, "a.txt", Access::Write).await.unwrap();
    vfs.write(handle, 0, b"hello").await.unwrap();
    vfs.release(handle);
    advance(settle()).await;
    yield_until("the create did not start", || provider.uploads() == 1).await;
    vfs.rename(Vfs::ROOT, "a.txt", Vfs::ROOT, "b.txt", IfExists::Refuse)
        .await
        .unwrap();
    advance(settle()).await;
    yield_until("the second create did not start", || {
        provider.uploads() == 2
    })
    .await;
    advance(Duration::from_secs(10)).await;
    yield_until("the renamed file did not land", || spool_files(&dir) == 0).await;
    let items = root_items(&provider).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "b.txt");
    assert_eq!(provider.contents(&items[0].id), b"hello");
}

#[tokio::test(start_paused = true)]
async fn aborting_the_uploader_keeps_the_spool_file_for_the_next_one() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let uploads = start_uploads(&vfs);
    provider.set_upload_delay(Duration::from_secs(10));
    let (_made, handle) = vfs.create(Vfs::ROOT, "a.txt", Access::Write).await.unwrap();
    vfs.write(handle, 0, b"hello").await.unwrap();
    vfs.release(handle);
    advance(settle()).await;
    yield_until("the create did not start", || provider.in_flight() == 1).await;
    uploads.abort();
    yield_until("the try was not stopped", || provider.in_flight() == 0).await;
    assert_eq!(spool_files(&dir), 1);
    assert!(root_items(&provider).await.is_empty());
    provider.set_upload_delay(Duration::ZERO);
    let _uploads = start_uploads(&vfs);
    yield_until("the next uploader did not send it", || {
        spool_files(&dir) == 0
    })
    .await;
    assert_eq!(root_items(&provider).await.len(), 1);
}

#[tokio::test(start_paused = true)]
async fn an_item_with_no_revision_is_never_overwritten() {
    let (provider, vfs, _dir) = spooled_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    let id = provider.add_file(&provider.root(), "a.txt", b"remote");
    provider.hide_revisions(true);
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    vfs.write(handle, 0, b"LOCAL!").await.unwrap();
    vfs.release(handle);
    advance(settle()).await;
    yield_until("the try did not fail", || {
        matches!(
            vfs.unsent().first().map(|unsent| unsent.state.clone()),
            Some(UnsentState::Refused(VfsError::NotSupported))
        )
    })
    .await;
    assert_eq!(provider.uploads(), 0);
    assert_eq!(provider.contents(&id), b"remote");
    let calls = provider.calls();
    advance(Duration::from_secs(10 * 60)).await;
    assert_eq!(provider.calls(), calls);
    assert!(matches!(
        vfs.unsent().first().map(|unsent| unsent.state.clone()),
        Some(UnsentState::Refused(VfsError::NotSupported))
    ));
}

#[tokio::test(start_paused = true)]
async fn an_item_renamed_out_of_a_local_name_uploads() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, finder_files);
    let _uploads = start_uploads(&vfs);
    provider.add_file(&provider.root(), "._a", b"old");
    let file = vfs.lookup(Vfs::ROOT, "._a").await.unwrap();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    vfs.write(handle, 0, b"new").await.unwrap();
    vfs.release(handle);
    advance(settle() + Duration::from_secs(1)).await;
    assert_eq!(provider.uploads(), 0);
    vfs.rename(Vfs::ROOT, "._a", Vfs::ROOT, "a.txt", IfExists::Refuse)
        .await
        .unwrap();
    advance(settle()).await;
    yield_until("the renamed file did not upload", || spool_files(&dir) == 0).await;
    let items = root_items(&provider).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "a.txt");
    assert_eq!(provider.contents(&items[0].id), b"new");
}

#[tokio::test(start_paused = true)]
async fn the_retry_wait_doubles_up_to_five_minutes() {
    let (provider, vfs, _dir) = spooled_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    let (_made, handle) = vfs.create(Vfs::ROOT, "a.txt", Access::Write).await.unwrap();
    vfs.write(handle, 0, b"hello").await.unwrap();
    vfs.release(handle);
    provider.fail_with(Some(VfsError::Offline));
    advance(settle()).await;
    yield_until("the first try did not start", || provider.uploads() == 1).await;
    for (tries, wait) in [5, 10, 20, 40, 80, 160, 300, 300].into_iter().enumerate() {
        advance(Duration::from_secs(wait) - Duration::from_millis(1)).await;
        assert_eq!(provider.uploads(), tries + 1, "retried before {wait} s");
        advance(Duration::from_millis(1)).await;
        yield_until("the retry did not start", || {
            provider.uploads() == tries + 2
        })
        .await;
    }
}

#[tokio::test(start_paused = true)]
async fn an_edit_starts_the_retry_count_again() {
    let (provider, vfs, _dir) = spooled_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    let (made, handle) = vfs.create(Vfs::ROOT, "a.txt", Access::Write).await.unwrap();
    vfs.write(handle, 0, b"hello").await.unwrap();
    vfs.release(handle);
    provider.fail_with(Some(VfsError::Offline));
    advance(settle()).await;
    yield_until("the first try did not start", || provider.uploads() == 1).await;
    advance(Duration::from_secs(5)).await;
    yield_until("the second try did not start", || provider.uploads() == 2).await;
    // The next wait would be 10 seconds. An edit puts it back to 5.
    let handle = vfs.open(made.node, Access::Write).await.unwrap();
    vfs.write(handle, 0, b"j").await.unwrap();
    vfs.release(handle);
    advance(settle()).await;
    yield_until("the edited file was not tried", || provider.uploads() == 3).await;
    advance(Duration::from_secs(5) - Duration::from_millis(1)).await;
    assert_eq!(provider.uploads(), 3);
    advance(Duration::from_millis(1)).await;
    yield_until("the retry after the edit did not come at 5 s", || {
        provider.uploads() == 4
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn a_discarded_save_over_a_file_can_be_written_again() {
    let (provider, vfs, _dir) = spooled_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    let id = provider.add_file(&provider.root(), "a.txt", b"old");
    vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let (made, handle) = vfs.create(Vfs::ROOT, "a.tmp", Access::Write).await.unwrap();
    vfs.write(handle, 0, b"new").await.unwrap();
    vfs.release(handle);
    vfs.rename(Vfs::ROOT, "a.tmp", Vfs::ROOT, "a.txt", IfExists::Replace)
        .await
        .unwrap();
    provider.fail_with(Some(VfsError::Offline));
    advance(settle()).await;
    yield_until("the save did not fail", || provider.uploads() == 1).await;
    vfs.discard_unsent(made.node).await.unwrap();
    provider.fail_with(None);
    // The node holds the item's answer, so it opens for write with no lookup first.
    let handle = vfs.open(made.node, Access::Write).await.unwrap();
    vfs.write(handle, 0, b"N").await.unwrap();
    vfs.release(handle);
    advance(settle()).await;
    yield_until("the second save did not land", || {
        provider.contents(&id) == b"Nld"
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn a_write_past_the_whole_spool_asks_the_provider_nothing() {
    let (provider, vfs, _dir) = spooled_mount(Options::default(), 1024, never_local);
    provider.add_file(&provider.root(), "a.txt", b"hello");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    advance(Options::default().attributes_ttl).await;
    provider.fail_with(Some(VfsError::Offline));
    let calls = provider.calls();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    assert_eq!(vfs.write(handle, 2000, b"x").await, Err(VfsError::NoSpace));
    assert_eq!(provider.calls(), calls);
}

#[tokio::test(start_paused = true)]
async fn removing_an_item_while_it_uploads_starts_no_second_upload() {
    let (provider, vfs, _dir) = spooled_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    provider.add_file(&provider.root(), "a.txt", b"old");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    vfs.write(handle, 0, b"new").await.unwrap();
    vfs.release(handle);
    // The replace would answer inside the remove's 5 seconds, find the item gone, and make it again, unless the remove stopped it first.
    provider.set_upload_delay(Duration::from_secs(2));
    advance(settle()).await;
    yield_until("the replace did not start", || provider.uploads() == 1).await;
    provider.set_delay(Duration::from_secs(5));
    vfs.remove(Vfs::ROOT, "a.txt", Kind::File).await.unwrap();
    advance(Duration::from_secs(60)).await;
    provider.set_delay(Duration::ZERO);
    assert_eq!(provider.uploads(), 1);
    assert!(root_items(&provider).await.is_empty());
}

#[tokio::test(start_paused = true)]
async fn first_writes_after_the_answer_went_stale_ask_for_the_item_once() {
    let (provider, vfs, _dir) = spooled_mount(Options::default(), 1024, never_local);
    provider.add_file(&provider.root(), "a.txt", b"0123456789");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    advance(Options::default().attributes_ttl).await;
    provider.set_delay(Duration::from_secs(1));
    let calls = provider.calls();
    let first = tokio::spawn({
        let vfs = Arc::clone(&vfs);
        async move { vfs.write(handle, 0, b"A").await }
    });
    let second = tokio::spawn({
        let vfs = Arc::clone(&vfs);
        async move { vfs.write(handle, 9, b"Z").await }
    });
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    // One `item` call and one read.
    assert_eq!(provider.calls(), calls + 2);
    assert_eq!(vfs.read(handle, 0, 10).await.unwrap(), b"A12345678Z");
}

#[tokio::test(start_paused = true)]
async fn a_save_over_a_file_bases_on_the_newest_answer_the_core_holds() {
    let options = Options {
        listing_ttl: Duration::from_secs(60),
        ..Options::default()
    };
    let (provider, vfs, dir) = spooled_mount(options, 1024, never_local);
    let _uploads = start_uploads(&vfs);
    let id = provider.add_file(&provider.root(), "a.txt", b"v1");
    let target = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    // The listing stays cached with the old revision. The node learns the new one.
    provider.write(&id, b"v2");
    advance(Options::default().attributes_ttl).await;
    vfs.attributes(target.node).await.unwrap();
    let (_made, handle) = vfs.create(Vfs::ROOT, "a.tmp", Access::Write).await.unwrap();
    vfs.write(handle, 0, b"v3").await.unwrap();
    vfs.release(handle);
    vfs.rename(Vfs::ROOT, "a.tmp", Vfs::ROOT, "a.txt", IfExists::Replace)
        .await
        .unwrap();
    advance(settle()).await;
    yield_until("the save did not land", || spool_files(&dir) == 0).await;
    let items = root_items(&provider).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, id);
    assert_eq!(provider.contents(&id), b"v3");
    assert_eq!(provider.uploads(), 1);
}

#[tokio::test]
async fn the_owner_calls_refuse_a_file_with_nothing_unsent() {
    let (provider, vfs, _dir) = spooled_mount(Options::default(), 1024, never_local);
    provider.add_file(&provider.root(), "a.txt", b"clean");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    assert_eq!(vfs.retry_now(file.node), Err(VfsError::NotFound));
    assert_eq!(vfs.discard_unsent(file.node).await, Err(VfsError::NotFound));
}

#[tokio::test(start_paused = true)]
async fn retry_now_leaves_a_waiting_file_its_settle_time() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    let (made, handle) = vfs.create(Vfs::ROOT, "a.txt", Access::Write).await.unwrap();
    vfs.write(handle, 0, b"hello").await.unwrap();
    vfs.release(handle);
    assert_eq!(vfs.retry_now(made.node), Ok(()));
    advance(settle() - Duration::from_millis(1)).await;
    assert_eq!(provider.uploads(), 0);
    advance(Duration::from_millis(1)).await;
    yield_until("the file did not upload", || spool_files(&dir) == 0).await;
}

#[tokio::test(start_paused = true)]
async fn a_failed_remove_sends_the_file_after_the_call_time_limit() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    let id = provider.add_file(&provider.root(), "a.txt", b"old");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    vfs.write(handle, 0, b"new").await.unwrap();
    vfs.release(handle);
    provider.set_upload_delay(Duration::from_secs(10));
    advance(settle()).await;
    yield_until("the replace did not start", || provider.uploads() == 1).await;
    provider.fail_with(Some(VfsError::Offline));
    assert_eq!(
        vfs.remove(Vfs::ROOT, "a.txt", Kind::File).await,
        Err(VfsError::Offline)
    );
    provider.fail_with(None);
    provider.set_upload_delay(Duration::ZERO);
    let call_timeout = Options::default().call_timeout;
    advance(call_timeout - Duration::from_millis(1)).await;
    assert_eq!(provider.uploads(), 1);
    advance(Duration::from_millis(1)).await;
    yield_until("the file was not sent after the remove failed", || {
        spool_files(&dir) == 0
    })
    .await;
    assert_eq!(provider.contents(&id), b"new");
}

#[tokio::test(start_paused = true)]
async fn an_upload_that_keeps_moving_outlives_the_call_time_limit() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    // Progress at 40 s is inside the 60-second stall time, and the whole upload takes far longer than the 30-second call time limit.
    provider.set_upload_delay(Duration::from_secs(80));
    let (_made, handle) = vfs.create(Vfs::ROOT, "a.txt", Access::Write).await.unwrap();
    vfs.write(handle, 0, b"hello").await.unwrap();
    vfs.release(handle);
    advance(settle()).await;
    yield_until("the upload did not start", || provider.uploads() == 1).await;
    advance(Duration::from_secs(80)).await;
    yield_until("the long upload did not land", || spool_files(&dir) == 0).await;
    assert_eq!(provider.uploads(), 1);
}

fn recovered(name: &str) -> bool {
    name.starts_with("a (recovered ") && name.ends_with(").txt")
}

async fn folder_names(vfs: &Vfs) -> Vec<String> {
    let handle = vfs.open_folder(Vfs::ROOT).unwrap();
    vfs.rewind_folder(handle).await.unwrap();
    let names = vfs
        .read_folder(handle, 0, 20)
        .unwrap()
        .into_iter()
        .map(|entry| entry.name)
        .collect();
    vfs.release(handle);
    names
}

async fn until_spool_empty(dir: &tempfile::TempDir) {
    yield_until("the spool folder was not emptied", || {
        spool_entries(dir) == 0
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn a_crash_with_a_writer_open_uploads_a_recovered_copy() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let id = provider.add_file(&provider.root(), "a.txt", b"old");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    vfs.write(handle, 0, b"new").await.unwrap();
    crash(vfs);
    let vfs = mount_on(&provider, &dir, Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    until_spool_empty(&dir).await;
    assert_eq!(provider.contents(&id), b"old");
    let items = root_items(&provider).await;
    assert_eq!(items.len(), 2);
    let original = items.iter().find(|item| item.id == id).unwrap();
    assert_eq!(original.name, "a.txt");
    let copy = items.iter().find(|item| item.id != id).unwrap();
    assert!(recovered(&copy.name));
    assert_eq!(provider.contents(&copy.id), b"new");
}

#[tokio::test(start_paused = true)]
async fn a_crash_after_fsync_uploads_the_synced_bytes_as_a_recovered_copy() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let id = provider.add_file(&provider.root(), "a.txt", b"old");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    vfs.write(handle, 0, b"synced").await.unwrap();
    vfs.fsync(file.node).await.unwrap();
    crash(vfs);
    let vfs = mount_on(&provider, &dir, Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    until_spool_empty(&dir).await;
    assert_eq!(provider.contents(&id), b"old");
    let items = root_items(&provider).await;
    let copy = items.iter().find(|item| recovered(&item.name)).unwrap();
    assert_eq!(provider.contents(&copy.id), b"synced");
}

#[tokio::test(start_paused = true)]
async fn a_crash_after_the_last_close_replaces_the_item() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let id = provider.add_file(&provider.root(), "a.txt", b"old");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    vfs.write(handle, 0, b"new").await.unwrap();
    let modified = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    vfs.set_modified(file.node, modified).await.unwrap();
    vfs.release(handle);
    crash(vfs);
    let vfs = mount_on(&provider, &dir, Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    until_spool_empty(&dir).await;
    assert_eq!(provider.uploads(), 1);
    assert_eq!(provider.contents(&id), b"new");
    let items = root_items(&provider).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, id);
    assert_eq!(items[0].attributes.modified, modified);
}

#[tokio::test(start_paused = true)]
async fn a_crash_after_the_last_close_creates_a_new_file_once() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let (_made, handle) = vfs.create(Vfs::ROOT, "a.txt", Access::Write).await.unwrap();
    vfs.write(handle, 0, b"hello").await.unwrap();
    vfs.release(handle);
    crash(vfs);
    let vfs = mount_on(&provider, &dir, Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    until_spool_empty(&dir).await;
    assert_eq!(provider.uploads(), 1);
    let items = root_items(&provider).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "a.txt");
    assert_eq!(provider.contents(&items[0].id), b"hello");
}

#[tokio::test(start_paused = true)]
async fn a_crash_after_a_save_through_a_temporary_file_replaces_the_item() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let id = provider.add_file(&provider.root(), "a.txt", b"old");
    let (_made, handle) = vfs
        .create(Vfs::ROOT, "a.txt.tmp", Access::Write)
        .await
        .unwrap();
    vfs.write(handle, 0, b"new").await.unwrap();
    vfs.release(handle);
    vfs.rename(
        Vfs::ROOT,
        "a.txt.tmp",
        Vfs::ROOT,
        "a.txt",
        IfExists::Replace,
    )
    .await
    .unwrap();
    crash(vfs);
    let vfs = mount_on(&provider, &dir, Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    until_spool_empty(&dir).await;
    assert_eq!(provider.contents(&id), b"new");
    let items = root_items(&provider).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, id);
    assert_eq!(items[0].name, "a.txt");
}

#[tokio::test(start_paused = true)]
async fn a_crash_mid_upload_sends_the_file_once() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let uploads = start_uploads(&vfs);
    provider.set_upload_delay(Duration::from_secs(60));
    let (_made, handle) = vfs.create(Vfs::ROOT, "a.txt", Access::Write).await.unwrap();
    vfs.write(handle, 0, b"hello").await.unwrap();
    vfs.release(handle);
    advance(settle()).await;
    yield_until("the upload did not start", || provider.uploads() == 1).await;
    uploads.abort();
    crash(vfs);
    assert!(root_items(&provider).await.is_empty());
    provider.set_upload_delay(Duration::ZERO);
    let vfs = mount_on(&provider, &dir, Options::default(), 1024, never_local);
    let started = provider.uploads();
    let _uploads = start_uploads(&vfs);
    until_spool_empty(&dir).await;
    assert_eq!(provider.uploads(), started + 1);
    let items = root_items(&provider).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "a.txt");
    assert_eq!(provider.contents(&items[0].id), b"hello");
}

#[tokio::test(start_paused = true)]
async fn a_crash_after_a_replace_lands_does_not_send_it_again() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let uploads = start_uploads(&vfs);
    provider.lose_upload_answers(true);
    let id = provider.add_file(&provider.root(), "a.txt", b"old");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    vfs.write(handle, 0, b"new").await.unwrap();
    vfs.release(handle);
    advance(settle()).await;
    yield_until("the replace did not land", || {
        provider.contents(&id) == b"new"
    })
    .await;
    uploads.abort();
    crash(vfs);
    provider.lose_upload_answers(false);
    let started = provider.uploads();
    let vfs = mount_on(&provider, &dir, Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    until_spool_empty(&dir).await;
    assert_eq!(provider.uploads(), started);
    let items = root_items(&provider).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, id);
    assert_eq!(provider.contents(&id), b"new");
}

#[tokio::test(start_paused = true)]
async fn a_crash_after_a_create_lands_does_not_create_it_again() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let uploads = start_uploads(&vfs);
    provider.lose_upload_answers(true);
    let (_made, handle) = vfs.create(Vfs::ROOT, "a.txt", Access::Write).await.unwrap();
    vfs.write(handle, 0, b"hello").await.unwrap();
    vfs.release(handle);
    advance(settle()).await;
    for _ in 0..1_000 {
        if provider.uploads() >= 1 && root_items(&provider).await.len() == 1 {
            break;
        }
        tokio::task::yield_now().await;
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(root_items(&provider).await.len(), 1);
    uploads.abort();
    crash(vfs);
    provider.lose_upload_answers(false);
    let started = provider.uploads();
    let vfs = mount_on(&provider, &dir, Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    until_spool_empty(&dir).await;
    assert_eq!(provider.uploads(), started);
    let items = root_items(&provider).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "a.txt");
    assert_eq!(provider.contents(&items[0].id), b"hello");
}

#[tokio::test(start_paused = true)]
async fn a_crash_after_a_conflict_copy_lands_leaves_one_copy() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let uploads = start_uploads(&vfs);
    let id = provider.add_file(&provider.root(), "a.txt", b"remote");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    vfs.write(handle, 0, b"local!").await.unwrap();
    vfs.release(handle);
    provider.write(&id, b"changed");
    provider.lose_upload_answers(true);
    advance(settle()).await;
    for _ in 0..1_000 {
        let copies = root_items(&provider)
            .await
            .into_iter()
            .filter(|item| item.name.starts_with("a (conflict ") && item.name.ends_with(").txt"))
            .count();
        if copies == 1 {
            break;
        }
        tokio::task::yield_now().await;
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(provider.uploads(), 2, "the replace and the conflict copy");
    uploads.abort();
    crash(vfs);
    provider.lose_upload_answers(false);
    let started = provider.uploads();
    let vfs = mount_on(&provider, &dir, Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    until_spool_empty(&dir).await;
    assert_eq!(provider.uploads(), started);
    assert_eq!(provider.contents(&id), b"changed");
    let copies: Vec<_> = root_items(&provider)
        .await
        .into_iter()
        .filter(|item| item.name.starts_with("a (conflict ") && item.name.ends_with(").txt"))
        .collect();
    assert_eq!(copies.len(), 1);
    assert_eq!(provider.contents(&copies[0].id), b"local!");
}

#[tokio::test(start_paused = true)]
async fn a_crash_after_the_upload_confirms_finds_nothing_to_send() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let uploads = start_uploads(&vfs);
    let (_made, handle) = vfs.create(Vfs::ROOT, "a.txt", Access::Write).await.unwrap();
    vfs.write(handle, 0, b"hello").await.unwrap();
    vfs.release(handle);
    advance(settle()).await;
    until_spool_empty(&dir).await;
    let started = provider.uploads();
    uploads.abort();
    crash(vfs);
    let vfs = mount_on(&provider, &dir, Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    advance(settle()).await;
    assert_eq!(provider.uploads(), started);
    assert_eq!(spool_entries(&dir), 0);
    drop(vfs);
}

#[tokio::test(start_paused = true)]
async fn a_crash_cleans_local_removed_and_half_written_records() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024 * 1024, finder_files);
    let (_good, good) = vfs.create(Vfs::ROOT, "a.txt", Access::Write).await.unwrap();
    vfs.write(good, 0, b"keep").await.unwrap();
    vfs.release(good);
    let (_local, local) = vfs
        .create(Vfs::ROOT, ".DS_Store", Access::Write)
        .await
        .unwrap();
    vfs.write(local, 0, b"junk").await.unwrap();
    vfs.release(local);
    let (_gone, gone) = vfs
        .create(Vfs::ROOT, "gone.txt", Access::Write)
        .await
        .unwrap();
    vfs.write(gone, 0, b"gone").await.unwrap();
    vfs.remove(Vfs::ROOT, "gone.txt", Kind::File).await.unwrap();
    std::fs::write(dir.path().join("1.record.new"), b"partial").unwrap();
    crash(vfs);
    let vfs = mount_on(
        &provider,
        &dir,
        Options::default(),
        1024 * 1024,
        finder_files,
    );
    let _uploads = start_uploads(&vfs);
    until_spool_empty(&dir).await;
    let items = root_items(&provider).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "a.txt");
    assert_eq!(provider.contents(&items[0].id), b"keep");
}

#[tokio::test(start_paused = true)]
async fn a_spool_file_with_no_record_is_kept_and_a_record_with_no_file_is_deleted() {
    let provider = Arc::new(MemoryProvider::new());
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("5"), b"bytes").unwrap();
    std::fs::write(dir.path().join("6"), b"").unwrap();
    std::fs::write(dir.path().join("7.record"), b"not a record").unwrap();
    let vfs = mount_on(&provider, &dir, Options::default(), 1024, never_local);
    let unplaced: Vec<_> = vfs
        .unplaced_spool_files()
        .iter()
        .map(|path| path.file_name().unwrap().to_str().unwrap().to_string())
        .collect();
    assert_eq!(unplaced, ["5", "6"]);
    let _uploads = start_uploads(&vfs);
    advance(settle()).await;
    assert_eq!(provider.uploads(), 0);
    assert_eq!(spool_files(&dir), 2);
    assert!(!dir.path().join("7.record").exists());
    assert!(dir.path().join("5").exists());
    assert!(dir.path().join("6").exists());
}

#[tokio::test(start_paused = true)]
async fn a_recovered_file_is_listed_before_it_uploads() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    provider.add_file(&provider.root(), "a.txt", b"old");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    vfs.write(handle, 0, b"new").await.unwrap();
    crash(vfs);
    let vfs = mount_on(&provider, &dir, Options::default(), 1024, never_local);
    let copy = folder_names(&vfs)
        .await
        .into_iter()
        .find(|name| recovered(name))
        .unwrap();
    vfs.lookup(Vfs::ROOT, &copy).await.unwrap();
    assert_eq!(provider.uploads(), 0);
}

#[tokio::test(start_paused = true)]
async fn fsync_of_a_clean_file_and_a_mount_without_a_spool_does_nothing() {
    let (provider, vfs) = mount(Options::default());
    provider.add_file(&provider.root(), "a.txt", b"hello");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    assert_eq!(vfs.fsync(file.node).await, Ok(()));
    let (provider, vfs, _dir) = spooled_mount(Options::default(), 1024, never_local);
    provider.add_file(&provider.root(), "a.txt", b"hello");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    assert_eq!(vfs.fsync(file.node).await, Ok(()));
}

#[tokio::test(start_paused = true)]
async fn a_stale_closed_record_uploads_a_recovered_copy() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let id = provider.add_file(&provider.root(), "a.txt", b"old");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    vfs.write(handle, 0, b"new").await.unwrap();
    vfs.release(handle);
    let saved = std::fs::read(dir.path().join("1.record")).unwrap();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    std::fs::write(dir.path().join("1.record"), &saved).unwrap();
    // Linux stamps a file's modified time from a clock that moves every few milliseconds. The same-length write must land on a later stamp.
    std::thread::sleep(Duration::from_millis(20));
    vfs.write(handle, 0, b"NEW").await.unwrap();
    crash(vfs);
    let vfs = mount_on(&provider, &dir, Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    until_spool_empty(&dir).await;
    assert_eq!(provider.contents(&id), b"old");
    let copy = root_items(&provider)
        .await
        .into_iter()
        .find(|item| recovered(&item.name))
        .unwrap();
    assert_eq!(provider.contents(&copy.id), b"NEW");
}

#[tokio::test(start_paused = true)]
async fn a_closed_file_that_never_started_a_try_does_not_take_another_clients_file() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let (made, handle) = vfs.create(Vfs::ROOT, "a.txt", Access::Write).await.unwrap();
    vfs.write(handle, 0, b"same").await.unwrap();
    // `add_file` stamps the other file one second after the epoch. The same stamp here means a check that ignored `tried` would take it.
    vfs.set_modified(made.node, UNIX_EPOCH + Duration::from_secs(1))
        .await
        .unwrap();
    vfs.release(handle);
    crash(vfs);
    provider.add_file(&provider.root(), "a.txt", b"same");
    let vfs = mount_on(&provider, &dir, Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    until_spool_empty(&dir).await;
    let items = root_items(&provider).await;
    assert_eq!(items.len(), 2);
    assert!(items.iter().all(|item| item.name == "a.txt"));
    assert!(items
        .iter()
        .all(|item| provider.contents(&item.id) == b"same"));
}

#[tokio::test(start_paused = true)]
async fn a_remote_name_holding_an_escaped_slash_is_placed_again() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let (_made, handle) = vfs
        .create(Vfs::ROOT, "a%2Fb.txt", Access::Write)
        .await
        .unwrap();
    vfs.write(handle, 0, b"hello").await.unwrap();
    vfs.release(handle);
    crash(vfs);
    let vfs = mount_on(&provider, &dir, Options::default(), 1024, never_local);
    vfs.lookup(Vfs::ROOT, "a%2Fb.txt").await.unwrap();
    let _uploads = start_uploads(&vfs);
    until_spool_empty(&dir).await;
    let items = root_items(&provider).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "a/b.txt");
    assert_eq!(provider.contents(&items[0].id), b"hello");
}

#[tokio::test(start_paused = true)]
async fn a_create_whose_record_cannot_be_written_leaves_no_file() {
    let (_provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    std::fs::create_dir(dir.path().join("1.record.new")).unwrap();
    assert!(vfs.create(Vfs::ROOT, "a.txt", Access::Write).await.is_err());
    assert_eq!(spool_files(&dir), 0);
    assert_eq!(
        vfs.lookup(Vfs::ROOT, "a.txt").await,
        Err(VfsError::NotFound)
    );
}

#[tokio::test(start_paused = true)]
async fn a_crash_after_a_gone_item_is_created_again_does_not_create_it_twice() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let uploads = start_uploads(&vfs);
    let id = provider.add_file(&provider.root(), "a.txt", b"old");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    vfs.write(handle, 0, b"new").await.unwrap();
    vfs.release(handle);
    provider.remote_remove(&id);
    provider.lose_upload_answers(true);
    advance(settle()).await;
    yield_until("the file was not created again", || provider.uploads() == 2).await;
    uploads.abort();
    crash(vfs);
    provider.lose_upload_answers(false);
    let vfs = mount_on(&provider, &dir, Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    until_spool_empty(&dir).await;
    assert_eq!(provider.uploads(), 2);
    let items = root_items(&provider).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "a.txt");
    assert_eq!(provider.contents(&items[0].id), b"new");
}

#[tokio::test(start_paused = true)]
async fn a_cut_frees_room_when_a_remount_lowers_the_limit_below_the_adopted_files() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let (_made, handle) = vfs.create(Vfs::ROOT, "a.txt", Access::Write).await.unwrap();
    vfs.write(handle, 0, &[7; 600]).await.unwrap();
    vfs.release(handle);
    crash(vfs);
    let vfs = mount_on(&provider, &dir, Options::default(), 100, never_local);
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    assert_eq!(vfs.truncate(file.node, 500).await.unwrap().size, 500);
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    vfs.write(handle, 200, b"x").await.unwrap();
    assert_eq!(vfs.read(handle, 199, 3).await.unwrap(), [7, b'x', 7]);
    assert_eq!(
        vfs.write(handle, 500, b"grow").await,
        Err(VfsError::NoSpace)
    );
}

#[tokio::test(start_paused = true)]
async fn a_first_write_whose_record_cannot_be_written_leaves_the_file_clean() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    provider.add_file(&provider.root(), "a.txt", b"old");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    std::fs::create_dir(dir.path().join("1.record.new")).unwrap();
    assert!(vfs.write(handle, 0, b"new").await.is_err());
    assert_eq!(spool_files(&dir), 0);
    assert_eq!(vfs.read(handle, 0, 3).await.unwrap(), b"old");
}

fn refused(vfs: &Vfs, error: VfsError) -> bool {
    matches!(
        vfs.unsent().first().map(|unsent| unsent.state.clone()),
        Some(UnsentState::Refused(found)) if found == error
    )
}

#[tokio::test]
async fn a_read_only_file_cannot_be_opened_for_write_or_cut() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let id = provider.add_file(&provider.root(), "theirs.txt", b"theirs");
    provider.set_read_only(&id, true);
    provider.add_file(&provider.root(), "mine.txt", b"mine");
    let theirs = vfs.lookup(Vfs::ROOT, "theirs.txt").await.unwrap();
    let mine = vfs.lookup(Vfs::ROOT, "mine.txt").await.unwrap();
    assert!(theirs.attributes.read_only);
    assert!(!mine.attributes.read_only);
    assert_eq!(
        vfs.open(theirs.node, Access::Write).await,
        Err(VfsError::AccessDenied)
    );
    assert_eq!(
        vfs.truncate(theirs.node, 0).await,
        Err(VfsError::AccessDenied)
    );
    let handle = vfs.open(theirs.node, Access::Read).await.unwrap();
    assert_eq!(vfs.read(handle, 0, 100).await.unwrap(), b"theirs");
    vfs.release(handle);
    assert_eq!(spool_files(&dir), 0);
    assert!(vfs.unsent().is_empty());
    assert_eq!(provider.uploads(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_new_file_renamed_over_a_read_only_file_is_refused() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    let id = provider.add_file(&provider.root(), "theirs.txt", b"theirs");
    provider.set_read_only(&id, true);
    let (made, handle) = vfs
        .create(Vfs::ROOT, "new.txt", Access::Write)
        .await
        .unwrap();
    vfs.write(handle, 0, b"new").await.unwrap();
    vfs.release(handle);
    assert_eq!(
        vfs.rename(
            Vfs::ROOT,
            "new.txt",
            Vfs::ROOT,
            "theirs.txt",
            IfExists::Replace
        )
        .await,
        Err(VfsError::AccessDenied)
    );
    let new_file = vfs.lookup(Vfs::ROOT, "new.txt").await.unwrap();
    let theirs = vfs.lookup(Vfs::ROOT, "theirs.txt").await.unwrap();
    assert_eq!(new_file.node, made.node);
    assert_ne!(theirs.node, made.node);
    advance(settle()).await;
    yield_until("the new file did not upload", || spool_files(&dir) == 0).await;
    assert_eq!(provider.contents(&id), b"theirs");
    let items = root_items(&provider).await;
    let new_item = items.iter().find(|item| item.name == "new.txt").unwrap();
    assert_eq!(provider.contents(&new_item.id), b"new");
    assert_ne!(new_item.id, id);
}

#[tokio::test(start_paused = true)]
async fn the_newest_answer_decides_whether_a_rename_over_a_file_is_refused() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let id = provider.add_file(&provider.root(), "theirs.txt", b"theirs");
    let looked = vfs.lookup(Vfs::ROOT, "theirs.txt").await.unwrap();
    assert!(!looked.attributes.read_only);
    provider.set_read_only(&id, true);
    vfs.refresh();
    let (_made, handle) = vfs
        .create(Vfs::ROOT, "new.txt", Access::Write)
        .await
        .unwrap();
    vfs.write(handle, 0, b"NEW").await.unwrap();
    vfs.release(handle);
    assert_eq!(
        vfs.rename(
            Vfs::ROOT,
            "new.txt",
            Vfs::ROOT,
            "theirs.txt",
            IfExists::Replace
        )
        .await,
        Err(VfsError::AccessDenied)
    );
    provider.set_read_only(&id, false);
    vfs.refresh();
    vfs.rename(
        Vfs::ROOT,
        "new.txt",
        Vfs::ROOT,
        "theirs.txt",
        IfExists::Replace,
    )
    .await
    .unwrap();
    assert_eq!(
        vfs.lookup(Vfs::ROOT, "new.txt").await.unwrap_err(),
        VfsError::NotFound
    );
    let _uploads = start_uploads(&vfs);
    advance(settle()).await;
    yield_until("the renamed file did not upload", || spool_files(&dir) == 0).await;
    assert_eq!(provider.contents(&id), b"NEW");
    let items = root_items(&provider).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, id);
}

#[tokio::test(start_paused = true)]
async fn a_refused_upload_stops_until_it_is_asked_again() {
    let (provider, vfs, _dir) = spooled_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    let (id, node) = refused_edit(&provider, &vfs).await;
    advance(Duration::from_secs(10 * 60)).await;
    assert_eq!(provider.uploads(), 1);
    assert!(refused(&vfs, VfsError::AccessDenied));
    let handle = vfs.open(node, Access::Read).await.unwrap();
    assert_eq!(vfs.read(handle, 0, 100).await.unwrap(), b"LOCAL!");
    vfs.release(handle);
    let handle = vfs.open(node, Access::Write).await.unwrap();
    vfs.release(handle);
    advance(settle()).await;
    assert_eq!(provider.uploads(), 1);
    vfs.retry_now(node).unwrap();
    yield_until("the retry was not refused", || {
        provider.uploads() == 2 && refused(&vfs, VfsError::AccessDenied)
    })
    .await;
    provider.set_read_only(&id, false);
    vfs.retry_now(node).unwrap();
    yield_until("the file did not upload", || vfs.unsent().is_empty()).await;
    assert_eq!(provider.contents(&id), b"LOCAL!");
}

#[tokio::test(start_paused = true)]
async fn an_edit_retries_a_refused_file() {
    let (provider, vfs, _dir) = spooled_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    let (id, node) = refused_edit(&provider, &vfs).await;
    provider.set_read_only(&id, false);
    let handle = vfs.open(node, Access::Write).await.unwrap();
    vfs.write(handle, 6, b"!").await.unwrap();
    vfs.release(handle);
    advance(settle()).await;
    yield_until("the edited file did not upload", || vfs.unsent().is_empty()).await;
    assert_eq!(provider.contents(&id), b"LOCAL!!");
}

#[tokio::test(start_paused = true)]
async fn renaming_a_refused_new_file_tries_it_again() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    // `create` lists the parent first. The refusal is the provider's create, so it is set after the file is local.
    let (_made, handle) = vfs.create(Vfs::ROOT, "a.txt", Access::Write).await.unwrap();
    vfs.write(handle, 0, b"hello").await.unwrap();
    vfs.release(handle);
    provider.fail_with(Some(VfsError::AccessDenied));
    advance(settle()).await;
    yield_until("the create was not refused", || {
        refused(&vfs, VfsError::AccessDenied)
    })
    .await;
    assert_eq!(provider.uploads(), 1);
    advance(Duration::from_secs(10 * 60)).await;
    assert_eq!(provider.uploads(), 1);
    assert!(refused(&vfs, VfsError::AccessDenied));
    provider.fail_with(None);
    vfs.rename(Vfs::ROOT, "a.txt", Vfs::ROOT, "b.txt", IfExists::Refuse)
        .await
        .unwrap();
    advance(settle()).await;
    yield_until("the renamed file did not upload", || spool_files(&dir) == 0).await;
    let items = root_items(&provider).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "b.txt");
    assert_eq!(provider.contents(&items[0].id), b"hello");
}

#[tokio::test]
async fn a_spooled_file_is_not_read_only() {
    let (provider, vfs, _dir) = spooled_mount(Options::default(), 1024, never_local);
    let id = provider.add_file(&provider.root(), "a.txt", b"remote");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    vfs.write(handle, 0, b"LOCAL").await.unwrap();
    provider.set_read_only(&id, true);
    assert!(!vfs.attributes(file.node).await.unwrap().read_only);
    vfs.release(handle);
}

/// A file whose edit the remote refused, as `a_refused_upload_stops_until_it_is_asked_again` makes it.
async fn refused_edit(provider: &MemoryProvider, vfs: &Vfs) -> (ItemId, NodeId) {
    let id = provider.add_file(&provider.root(), "a.txt", b"remote");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    vfs.write(handle, 0, b"LOCAL!").await.unwrap();
    provider.set_read_only(&id, true);
    vfs.release(handle);
    advance(settle()).await;
    yield_until("the try was not refused", || {
        refused(vfs, VfsError::AccessDenied)
    })
    .await;
    assert_eq!(provider.uploads(), 1);
    (id, file.node)
}

#[tokio::test(start_paused = true)]
async fn a_refused_item_renamed_while_a_writer_is_open_is_sent_when_it_closes() {
    let (provider, vfs, _dir) = spooled_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    let (id, node) = refused_edit(&provider, &vfs).await;
    provider.set_read_only(&id, false);
    // The open writer holds back the try the rename asks for, and its close carries no edit.
    let writer = vfs.open(node, Access::Write).await.unwrap();
    vfs.rename(Vfs::ROOT, "a.txt", Vfs::ROOT, "b.txt", IfExists::Refuse)
        .await
        .unwrap();
    advance(settle()).await;
    assert_eq!(provider.uploads(), 1);
    vfs.release(writer);
    advance(settle()).await;
    yield_until("the renamed file was not sent", || vfs.unsent().is_empty()).await;
    assert_eq!(provider.contents(&id), b"LOCAL!");
}

#[tokio::test(start_paused = true)]
async fn a_new_modified_time_sends_a_refused_file_again() {
    let (provider, vfs, _dir) = spooled_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    let (id, node) = refused_edit(&provider, &vfs).await;
    provider.set_read_only(&id, false);
    // `touch`, `cp -p`, and `rsync -t` set only the time.
    vfs.set_modified(node, UNIX_EPOCH + Duration::from_secs(5))
        .await
        .unwrap();
    advance(settle()).await;
    yield_until("the file was not sent again", || vfs.unsent().is_empty()).await;
    assert_eq!(provider.contents(&id), b"LOCAL!");
}

#[tokio::test(start_paused = true)]
async fn a_denied_remove_does_not_send_a_refused_file() {
    let (provider, vfs, _dir) = spooled_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    refused_edit(&provider, &vfs).await;
    // A fresh listing, so the remove reaches the provider's own call.
    vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    provider.fail_with(Some(VfsError::AccessDenied));
    assert_eq!(
        vfs.remove(Vfs::ROOT, "a.txt", Kind::File).await,
        Err(VfsError::AccessDenied)
    );
    provider.fail_with(None);
    advance(Options::default().call_timeout + settle()).await;
    assert_eq!(provider.uploads(), 1);
    assert!(refused(&vfs, VfsError::AccessDenied));
}

#[tokio::test(start_paused = true)]
async fn a_first_write_after_the_remote_marks_the_file_read_only_is_refused() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let id = provider.add_file(&provider.root(), "a.txt", b"remote");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    provider.set_read_only(&id, true);
    // The first write asks for the item again once the answer the open used is stale.
    advance(Options::default().attributes_ttl).await;
    assert_eq!(
        vfs.write(handle, 0, b"LOCAL").await,
        Err(VfsError::AccessDenied)
    );
    assert_eq!(spool_files(&dir), 0);
    assert!(vfs.unsent().is_empty());
    vfs.release(handle);
}

#[tokio::test(start_paused = true)]
async fn a_first_write_refuses_a_mark_a_fresh_listing_brought() {
    let (provider, vfs, dir) = spooled_mount(Options::default(), 1024, never_local);
    let id = provider.add_file(&provider.root(), "a.txt", b"remote");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    provider.set_read_only(&id, true);
    vfs.refresh();
    assert!(
        vfs.lookup(Vfs::ROOT, "a.txt")
            .await
            .unwrap()
            .attributes
            .read_only
    );
    let calls = provider.calls();
    assert_eq!(
        vfs.write(handle, 0, b"LOCAL").await,
        Err(VfsError::AccessDenied)
    );
    assert_eq!(provider.calls(), calls);
    assert_eq!(spool_files(&dir), 0);
    vfs.release(handle);
}

#[tokio::test]
async fn a_folder_renamed_when_ids_are_paths_keeps_working_for_the_nodes_inside_it() {
    let (provider, vfs) = path_mount(Options::default());
    provider.add_folder(&provider.root(), "docs");
    provider.add_folder(&path("/docs"), "sub");
    provider.add_file(&path("/docs/sub"), "a.txt", b"hello");
    let docs = vfs.lookup(Vfs::ROOT, "docs").await.unwrap();
    let sub = vfs.lookup(docs.node, "sub").await.unwrap();
    let file = vfs.lookup(sub.node, "a.txt").await.unwrap();
    let read = vfs.open(file.node, Access::Read).await.unwrap();
    let folder = vfs.open_folder(sub.node).unwrap();
    vfs.rename(Vfs::ROOT, "docs", Vfs::ROOT, "papers", IfExists::Replace)
        .await
        .unwrap();
    let found = vfs.lookup(sub.node, "a.txt").await.unwrap();
    assert_eq!(found.node, file.node);
    assert_eq!(vfs.read(read, 0, 5).await.unwrap(), b"hello");
    vfs.rewind_folder(folder).await.unwrap();
    let listed = vfs.read_folder(folder, 0, 10).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, "a.txt");
    assert_eq!(listed[0].node, file.node);
    assert_eq!(vfs.attributes(sub.node).await.unwrap().kind, Kind::Folder);
}

#[tokio::test(start_paused = true)]
async fn a_spooled_file_inside_a_renamed_folder_uploads_to_its_new_path() {
    let (provider, vfs, dir) = spooled_path_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    provider.add_folder(&provider.root(), "docs");
    provider.add_file(&path("/docs"), "a.txt", b"old");
    let docs = vfs.lookup(Vfs::ROOT, "docs").await.unwrap();
    let file = vfs.lookup(docs.node, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    vfs.write(handle, 0, b"new").await.unwrap();
    vfs.release(handle);
    vfs.rename(Vfs::ROOT, "docs", Vfs::ROOT, "papers", IfExists::Replace)
        .await
        .unwrap();
    advance(settle()).await;
    yield_until("the file did not upload to its new path", || {
        spool_files(&dir) == 0
    })
    .await;
    assert_eq!(provider.contents(&path("/papers/a.txt")), b"new");
    let items = root_items(&provider).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "papers");
}

#[tokio::test(start_paused = true)]
async fn new_files_deep_inside_a_renamed_folder_follow_it_and_upload_there() {
    let (provider, vfs, dir) = spooled_path_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    provider.add_folder(&provider.root(), "docs");
    provider.add_folder(&path("/docs"), "sub");
    let docs = vfs.lookup(Vfs::ROOT, "docs").await.unwrap();
    let sub = vfs.lookup(docs.node, "sub").await.unwrap();
    let (made, handle) = vfs
        .create(sub.node, "new.txt", Access::Write)
        .await
        .unwrap();
    vfs.write(handle, 0, b"hi").await.unwrap();
    vfs.release(handle);
    vfs.rename(Vfs::ROOT, "docs", Vfs::ROOT, "papers", IfExists::Replace)
        .await
        .unwrap();
    let papers = vfs.lookup(Vfs::ROOT, "papers").await.unwrap();
    let sub = vfs.lookup(papers.node, "sub").await.unwrap();
    let found = vfs.lookup(sub.node, "new.txt").await.unwrap();
    assert_eq!(found.node, made.node);
    advance(settle()).await;
    yield_until("the new file did not upload", || spool_files(&dir) == 0).await;
    assert_eq!(provider.contents(&path("/papers/sub/new.txt")), b"hi");
    let items = root_items(&provider).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "papers");
}

#[tokio::test]
async fn after_a_folder_is_renamed_a_folder_made_under_its_old_name_is_empty() {
    let (provider, vfs) = path_mount(Options::default());
    provider.add_folder(&provider.root(), "a");
    provider.add_file(&path("/a"), "x.txt", b"x");
    let old = vfs.lookup(Vfs::ROOT, "a").await.unwrap();
    vfs.lookup(old.node, "x.txt").await.unwrap();
    vfs.rename(Vfs::ROOT, "a", Vfs::ROOT, "b", IfExists::Replace)
        .await
        .unwrap();
    let made = vfs.create_folder(Vfs::ROOT, "a").await.unwrap();
    assert_ne!(made.node, old.node);
    assert_eq!(
        vfs.lookup(made.node, "x.txt").await,
        Err(VfsError::NotFound)
    );
    assert_eq!(vfs.lookup(old.node, "x.txt").await.unwrap().name, "x.txt");
}

#[tokio::test]
async fn a_file_removed_and_made_again_is_a_new_node_and_the_old_handle_is_stale() {
    let (provider, vfs, _dir) = spooled_path_mount(Options::default(), 1024, never_local);
    provider.add_file(&provider.root(), "f.txt", b"old");
    let file = vfs.lookup(Vfs::ROOT, "f.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Read).await.unwrap();
    vfs.remove(Vfs::ROOT, "f.txt", Kind::File).await.unwrap();
    let (made, write) = vfs.create(Vfs::ROOT, "f.txt", Access::Write).await.unwrap();
    vfs.write(write, 0, b"new").await.unwrap();
    assert_ne!(made.node, file.node);
    assert_eq!(vfs.read(handle, 0, 3).await, Err(VfsError::StaleHandle));
}

#[tokio::test]
async fn a_path_another_client_makes_again_is_never_read_through_a_removed_node() {
    let (provider, vfs) = path_mount(Options::default());
    provider.add_file(&provider.root(), "g.txt", b"old");
    let file = vfs.lookup(Vfs::ROOT, "g.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Read).await.unwrap();
    vfs.remove(Vfs::ROOT, "g.txt", Kind::File).await.unwrap();
    provider.add_file(&provider.root(), "g.txt", b"new");
    vfs.refresh();
    assert_eq!(vfs.read(handle, 0, 3).await, Err(VfsError::StaleHandle));
}

#[tokio::test]
async fn a_handle_on_a_file_replaced_by_a_rename_never_reads_the_moved_files_bytes() {
    let (provider, vfs) = path_mount(Options::default());
    provider.add_file(&provider.root(), "a.txt", b"AAA");
    provider.add_file(&provider.root(), "b.txt", b"BBB");
    let moved = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let replaced = vfs.lookup(Vfs::ROOT, "b.txt").await.unwrap();
    let handle = vfs.open(replaced.node, Access::Read).await.unwrap();
    vfs.rename(Vfs::ROOT, "a.txt", Vfs::ROOT, "b.txt", IfExists::Replace)
        .await
        .unwrap();
    assert_eq!(vfs.read(handle, 0, 3).await, Err(VfsError::StaleHandle));
    let found = vfs.lookup(Vfs::ROOT, "b.txt").await.unwrap();
    assert_eq!(found.node, moved.node);
    let read = vfs.open(found.node, Access::Read).await.unwrap();
    assert_eq!(vfs.read(read, 0, 3).await.unwrap(), b"AAA");

    let (provider, vfs, _dir) = spooled_path_mount(Options::default(), 1024, never_local);
    provider.add_file(&provider.root(), "c.txt", b"CCC");
    provider.add_file(&provider.root(), "d.txt", b"DDD");
    let kept = vfs.lookup(Vfs::ROOT, "d.txt").await.unwrap();
    let write = vfs.open(kept.node, Access::Write).await.unwrap();
    vfs.write(write, 0, b"XYZ").await.unwrap();
    vfs.rename(Vfs::ROOT, "c.txt", Vfs::ROOT, "d.txt", IfExists::Replace)
        .await
        .unwrap();
    assert_eq!(vfs.read(write, 0, 3).await.unwrap(), b"XYZ");
}

#[tokio::test(start_paused = true)]
async fn a_listing_in_flight_across_a_folder_rename_is_not_kept() {
    let (provider, vfs) = path_mount(Options::default());
    provider.add_folder(&provider.root(), "docs");
    provider.add_file(&path("/docs"), "x.txt", b"x");
    let docs = vfs.lookup(Vfs::ROOT, "docs").await.unwrap();
    let handle = vfs.open_folder(docs.node).unwrap();
    provider.set_delay(Duration::from_secs(10));
    let vfs_rewind = Arc::clone(&vfs);
    let rewind = tokio::spawn(async move { vfs_rewind.rewind_folder(handle).await });
    until_in_flight(&provider, 1).await;
    provider.set_delay(Duration::ZERO);
    vfs.rename(Vfs::ROOT, "docs", Vfs::ROOT, "papers", IfExists::Replace)
        .await
        .unwrap();
    let made = vfs.create_folder(Vfs::ROOT, "docs").await.unwrap();
    advance(Duration::from_secs(10)).await;
    rewind.await.unwrap().unwrap();
    assert_eq!(
        vfs.lookup(made.node, "x.txt").await,
        Err(VfsError::NotFound)
    );
    let papers = vfs.lookup(Vfs::ROOT, "papers").await.unwrap();
    let file = vfs.lookup(papers.node, "x.txt").await.unwrap();
    let listed = vfs.read_folder(handle, 0, 10).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, "x.txt");
    assert_eq!(listed[0].node, file.node);
}

#[tokio::test(start_paused = true)]
async fn a_folder_made_where_another_client_moved_one_is_a_new_node_with_no_listing() {
    let (provider, vfs) = path_mount(Options::default());
    provider.add_folder(&provider.root(), "a");
    provider.add_file(&path("/a"), "x.txt", b"x");
    let old = vfs.lookup(Vfs::ROOT, "a").await.unwrap();
    advance(Duration::from_secs(3)).await;
    let inside = vfs.lookup(old.node, "x.txt").await.unwrap();
    let handle = vfs.open(inside.node, Access::Read).await.unwrap();
    provider.remote_rename(&path("/a"), &provider.root(), "b");
    advance(Duration::from_millis(2_500)).await;
    let made = vfs.create_folder(Vfs::ROOT, "a").await.unwrap();
    assert_ne!(made.node, old.node);
    // The node inside the old folder lets go of its path too.
    assert_eq!(vfs.read(handle, 0, 1).await, Err(VfsError::StaleHandle));
    assert_eq!(
        vfs.lookup(made.node, "x.txt").await,
        Err(VfsError::NotFound)
    );
    assert_eq!(
        vfs.lookup(old.node, "x.txt").await,
        Err(VfsError::StaleHandle)
    );
}

#[tokio::test]
async fn a_removed_file_keeps_its_id_on_a_provider_whose_ids_stay_with_their_items() {
    let (provider, vfs, _dir) = spooled_mount(Options::default(), 1024, never_local);
    provider.add_file(&provider.root(), "a.txt", b"old");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Read).await.unwrap();
    vfs.remove(Vfs::ROOT, "a.txt", Kind::File).await.unwrap();
    assert_eq!(vfs.read(handle, 0, 3).await, Err(VfsError::NotFound));
}

#[tokio::test(start_paused = true)]
async fn a_spooled_file_becomes_a_conflict_copy_when_another_item_takes_its_path() {
    let (provider, vfs, dir) = spooled_path_mount(Options::default(), 1024, never_local);
    provider.add_file(&provider.root(), "a.txt", b"old");
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    vfs.write(handle, 0, b"mine").await.unwrap();
    provider.remote_remove(&path("/a.txt"));
    vfs.refresh();
    let made = vfs.create_folder(Vfs::ROOT, "a.txt").await.unwrap();
    assert_ne!(made.node, file.node);
    assert_eq!(vfs.read(handle, 0, 4).await.unwrap(), b"mine");
    let unsent = vfs.unsent();
    assert_eq!(unsent.len(), 1);
    assert_eq!(unsent[0].node, file.node);
    assert!(
        unsent[0].name.starts_with("a (conflict "),
        "{}",
        unsent[0].name
    );
    vfs.release(handle);
    // The next mount of the spool finds the copy, not a file at the folder's path.
    drop(vfs);
    let vfs = mount_on(&provider, &dir, Options::default(), 1024, never_local);
    let folder = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    assert_eq!(folder.attributes.kind, Kind::Folder);
    let _uploads = start_uploads(&vfs);
    advance(settle()).await;
    yield_until("the copy did not upload", || spool_files(&dir) == 0).await;
    let items = root_items(&provider).await;
    assert_eq!(items.len(), 2);
    let copy = items
        .iter()
        .find(|item| item.name.starts_with("a (conflict "))
        .unwrap();
    assert_eq!(provider.contents(&copy.id), b"mine");
}

#[tokio::test(start_paused = true)]
async fn a_create_that_lands_before_its_folder_is_renamed_is_kept_at_the_new_path() {
    let (provider, vfs, dir) = spooled_path_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    provider.add_folder(&provider.root(), "docs");
    provider.set_upload_delay(Duration::from_secs(5));
    let docs = vfs.lookup(Vfs::ROOT, "docs").await.unwrap();
    let (made, handle) = vfs
        .create(docs.node, "new.txt", Access::Write)
        .await
        .unwrap();
    vfs.write(handle, 0, b"hi").await.unwrap();
    vfs.release(handle);
    advance(settle()).await;
    yield_until("the upload did not start", || provider.uploads() == 1).await;
    vfs.refresh();
    provider.set_delay(Duration::from_secs(10));
    let vfs_rename = Arc::clone(&vfs);
    let renamed = tokio::spawn(async move {
        vfs_rename
            .rename(Vfs::ROOT, "docs", Vfs::ROOT, "papers", IfExists::Replace)
            .await
    });
    advance(Duration::from_secs(20)).await;
    renamed.await.unwrap().unwrap();
    yield_until("the spool file was not dropped", || spool_files(&dir) == 0).await;
    assert_eq!(provider.uploads(), 1);
    let items = root_items(&provider).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "papers");
    assert_eq!(provider.contents(&path("/papers/new.txt")), b"hi");
    let papers = vfs.lookup(Vfs::ROOT, "papers").await.unwrap();
    let found = vfs.lookup(papers.node, "new.txt").await.unwrap();
    assert_eq!(found.node, made.node);
    assert_eq!(found.attributes.size, 2);
}

#[tokio::test(start_paused = true)]
async fn a_first_write_never_mixes_bytes_from_two_files_at_one_path() {
    // The core fetches a remote file in pieces of this size.
    const PIECE: usize = 4 * 1024 * 1024;
    let (provider, vfs, _dir) =
        spooled_path_mount(Options::default(), 64 * 1024 * 1024, never_local);
    provider.add_file(&provider.root(), "a.txt", &vec![b'a'; PIECE + 10]);
    let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    provider.set_delay(Duration::from_secs(10));
    let vfs_write = Arc::clone(&vfs);
    let writing = tokio::spawn(async move { vfs_write.write(handle, 0, b"X").await });
    // The fetch creates its spool file on a blocking thread before the first read.
    yield_until("the fetch did not start", || provider.in_flight() >= 1).await;
    provider.set_delay(Duration::ZERO);
    vfs.rename(Vfs::ROOT, "a.txt", Vfs::ROOT, "b.txt", IfExists::Replace)
        .await
        .unwrap();
    provider.add_file(&provider.root(), "a.txt", &[b'z'; 20]);
    advance(Duration::from_secs(10)).await;
    writing.await.unwrap().unwrap();
    let read = vfs.read(handle, 0, (PIECE + 10) as u32).await.unwrap();
    assert_eq!(read.len(), PIECE + 10);
    assert_eq!(read[0], b'X');
    assert!(read[1..].iter().all(|byte| *byte == b'a'));
    assert_eq!(
        vfs.attributes(file.node).await.unwrap().size,
        (PIECE + 10) as u64
    );
}

#[tokio::test(start_paused = true)]
async fn a_refused_upload_is_sent_again_when_its_folder_moves() {
    let (provider, vfs, dir) = spooled_path_mount(Options::default(), 1024, never_local);
    let _uploads = start_uploads(&vfs);
    provider.add_folder(&provider.root(), "docs");
    let docs = vfs.lookup(Vfs::ROOT, "docs").await.unwrap();
    let (_made, handle) = vfs
        .create(docs.node, "new.txt", Access::Write)
        .await
        .unwrap();
    vfs.write(handle, 0, b"hi").await.unwrap();
    provider.fail_with(Some(VfsError::AccessDenied));
    vfs.release(handle);
    advance(settle()).await;
    yield_until("the upload was not refused", || {
        matches!(
            vfs.unsent().first().map(|unsent| unsent.state.clone()),
            Some(UnsentState::Refused(_))
        )
    })
    .await;
    provider.fail_with(None);
    vfs.rename(Vfs::ROOT, "docs", Vfs::ROOT, "papers", IfExists::Replace)
        .await
        .unwrap();
    advance(settle()).await;
    yield_until("the file was not sent again", || spool_files(&dir) == 0).await;
    assert_eq!(provider.contents(&path("/papers/new.txt")), b"hi");
}

#[tokio::test(start_paused = true)]
async fn a_replace_confirmed_after_its_file_is_renamed_shows_the_new_name() {
    // Paths, and ids that a rename changes for the moved item only, as Google Drive's do for a file moved between My Drive and a shared drive.
    let fresh_ids = MemoryProvider::new();
    fresh_ids.rename_changes_ids(true);
    for provider in [MemoryProvider::with_path_ids(), fresh_ids] {
        let options = Options {
            listing_ttl: Duration::from_secs(60),
            ..Options::default()
        };
        let (provider, vfs, dir) = spooled_with(provider, options, 1024, never_local);
        let _uploads = start_uploads(&vfs);
        provider.set_upload_delay(Duration::from_secs(5));
        provider.add_file(&provider.root(), "a.txt", b"old");
        let file = vfs.lookup(Vfs::ROOT, "a.txt").await.unwrap();
        let handle = vfs.open(file.node, Access::Write).await.unwrap();
        vfs.write(handle, 0, b"new").await.unwrap();
        vfs.release(handle);
        advance(settle()).await;
        yield_until("the replace did not start", || provider.uploads() == 1).await;
        // The rename lists the root for 10 seconds while it holds the changes lock, so the replace lands at 5 seconds and its confirm waits for the rename.
        vfs.refresh();
        provider.set_delay(Duration::from_secs(10));
        let vfs_rename = Arc::clone(&vfs);
        let renamed = tokio::spawn(async move {
            vfs_rename
                .rename(Vfs::ROOT, "a.txt", Vfs::ROOT, "b.txt", IfExists::Replace)
                .await
        });
        advance(Duration::from_secs(20)).await;
        renamed.await.unwrap().unwrap();
        yield_until("the spool file was not dropped", || spool_files(&dir) == 0).await;
        provider.set_delay(Duration::ZERO);
        assert_eq!(provider.uploads(), 1);
        let items = root_items(&provider).await;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "b.txt");
        assert_eq!(provider.contents(&items[0].id), b"new");
        let calls = provider.calls();
        assert_eq!(
            vfs.lookup(Vfs::ROOT, "b.txt").await.unwrap().node,
            file.node
        );
        // The cached listing answered, with the new name.
        assert_eq!(provider.calls(), calls);
    }
}

#[tokio::test]
async fn a_rename_that_changes_the_id_keeps_the_node_when_ids_are_not_paths() {
    let (provider, vfs) = mount(Options::default());
    provider.rename_changes_ids(true);
    let docs = provider.add_folder(&provider.root(), "docs");
    provider.add_file(&docs, "a.txt", b"hello");
    let folder = vfs.lookup(Vfs::ROOT, "docs").await.unwrap();
    let file = vfs.lookup(folder.node, "a.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Read).await.unwrap();
    vfs.rename(Vfs::ROOT, "docs", Vfs::ROOT, "papers", IfExists::Replace)
        .await
        .unwrap();
    let moved = vfs.lookup(Vfs::ROOT, "papers").await.unwrap();
    assert_eq!(moved.node, folder.node);
    assert_eq!(
        vfs.lookup(folder.node, "a.txt").await.unwrap().node,
        file.node
    );
    assert_eq!(vfs.read(handle, 0, 5).await.unwrap(), b"hello");
}

#[tokio::test(start_paused = true)]
async fn a_spooled_file_that_stays_local_is_retired_when_another_item_takes_its_path() {
    let (provider, vfs, dir) = spooled_path_mount(Options::default(), 1024, finder_files);
    let _uploads = start_uploads(&vfs);
    provider.add_file(&provider.root(), ".DS_Store", b"old");
    let file = vfs.lookup(Vfs::ROOT, ".DS_Store").await.unwrap();
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    vfs.write(handle, 0, b"mine").await.unwrap();
    provider.remote_remove(&path("/.DS_Store"));
    vfs.refresh();
    vfs.create_folder(Vfs::ROOT, ".DS_Store").await.unwrap();
    assert_eq!(vfs.read(handle, 0, 4).await.unwrap(), b"mine");
    vfs.release(handle);
    vfs.forget(file.node, 1);
    advance(settle()).await;
    yield_until("the spool file was not deleted", || spool_files(&dir) == 0).await;
    let items = root_items(&provider).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].attributes.kind, Kind::Folder);
    assert_eq!(provider.uploads(), 0);
}

#[tokio::test]
async fn a_rename_leaves_a_folder_whose_name_starts_the_same_alone() {
    let (provider, vfs) = path_mount(Options::default());
    provider.add_folder(&provider.root(), "a");
    provider.add_folder(&provider.root(), "ab");
    provider.add_file(&path("/ab"), "y.txt", b"y");
    let ab = vfs.lookup(Vfs::ROOT, "ab").await.unwrap();
    let file = vfs.lookup(ab.node, "y.txt").await.unwrap();
    let handle = vfs.open(file.node, Access::Read).await.unwrap();
    vfs.rename(Vfs::ROOT, "a", Vfs::ROOT, "c", IfExists::Replace)
        .await
        .unwrap();
    let calls = provider.calls();
    assert_eq!(vfs.lookup(ab.node, "y.txt").await.unwrap().node, file.node);
    // The listing of `/ab` is still cached.
    assert_eq!(provider.calls(), calls);
    assert_eq!(vfs.read(handle, 0, 1).await.unwrap(), b"y");
    assert_eq!(provider.contents(&path("/ab/y.txt")), b"y");
}

#[tokio::test]
async fn a_rename_onto_a_path_another_client_freed_retires_the_node_left_there() {
    let (provider, vfs) = path_mount(Options::default());
    provider.add_file(&provider.root(), "a.txt", b"AAA");
    provider.add_file(&provider.root(), "b.txt", b"BBB");
    let old = vfs.lookup(Vfs::ROOT, "b.txt").await.unwrap();
    let handle = vfs.open(old.node, Access::Read).await.unwrap();
    provider.remote_remove(&path("/b.txt"));
    vfs.refresh();
    // The rename finds `a.txt` in the listing, so no node holds the source.
    vfs.rename(Vfs::ROOT, "a.txt", Vfs::ROOT, "b.txt", IfExists::Refuse)
        .await
        .unwrap();
    assert_eq!(vfs.read(handle, 0, 3).await, Err(VfsError::StaleHandle));
}

/// Forwards every call and can write, but has no [`Provider::copy_file`].
struct NoCopy(Arc<MemoryProvider>);

#[async_trait::async_trait]
impl Provider for NoCopy {
    fn root(&self) -> ItemId {
        self.0.root()
    }

    fn can_write(&self) -> bool {
        true
    }

    async fn item(&self, id: &ItemId) -> Result<Item, VfsError> {
        self.0.item(id).await
    }

    async fn list(&self, folder: &ItemId) -> Result<Vec<Item>, VfsError> {
        self.0.list(folder).await
    }

    async fn read(&self, id: &ItemId, offset: u64, length: u32) -> Result<Vec<u8>, VfsError> {
        self.0.read(id, offset, length).await
    }

    async fn space(&self) -> Result<Space, VfsError> {
        self.0.space().await
    }
}

#[tokio::test]
async fn a_folder_copy_leaves_out_a_new_file_whose_name_stays_on_this_computer() {
    let (provider, vfs, _dir) = spooled_mount(Options::default(), 1024 * 1024, finder_files);
    let album = provider.add_folder(&provider.root(), "Album");
    provider.add_file(&album, "a.jpg", b"aaa");
    let sub = provider.add_folder(&album, "Sub");
    provider.add_file(&sub, "b.jpg", b"bbb");
    let folder = vfs.lookup(Vfs::ROOT, "Album").await.unwrap();
    let (made, handle) = vfs
        .create(folder.node, ".DS_Store", Access::Write)
        .await
        .unwrap();
    vfs.write(handle, 0, b"store").await.unwrap();
    vfs.release(handle);
    vfs.forget(made.node, 1);

    let before: Vec<_> = root_items(&provider)
        .await
        .into_iter()
        .map(|item| item.name)
        .collect();
    assert_eq!(vfs.copy("Album", "._x").await.unwrap(), Copied::NotEligible);
    let after: Vec<_> = root_items(&provider)
        .await
        .into_iter()
        .map(|item| item.name)
        .collect();
    assert_eq!(after, before);

    assert_eq!(
        vfs.copy("Album", "Album copy").await.unwrap(),
        Copied::To("Album copy".into())
    );
    let copied = root_items(&provider)
        .await
        .into_iter()
        .find(|item| item.name == "Album copy")
        .unwrap();
    let children = provider.list(&copied.id).await.unwrap();
    let names: Vec<_> = children.iter().map(|item| item.name.as_str()).collect();
    assert_eq!(names, ["a.jpg", "Sub"]);
    let photo = children.iter().find(|item| item.name == "a.jpg").unwrap();
    assert_eq!(provider.contents(&photo.id), b"aaa");
    let nested = children.iter().find(|item| item.name == "Sub").unwrap();
    let inside = provider.list(&nested.id).await.unwrap();
    assert_eq!(inside.len(), 1);
    assert_eq!(inside[0].name, "b.jpg");
    assert_eq!(provider.contents(&inside[0].id), b"bbb");
}

#[tokio::test]
async fn a_provider_that_cannot_copy_is_not_eligible() {
    let memory = Arc::new(MemoryProvider::new());
    memory.add_file(&memory.root(), "a.txt", b"a");
    let dir = tempfile::tempdir().unwrap();
    let vfs = Vfs::new(
        Arc::new(NoCopy(Arc::clone(&memory))),
        POSIX,
        Options::default(),
    )
    .take_writes(Spool {
        folder: dir.path().to_path_buf(),
        limit: 1024,
        stays_local: never_local,
    })
    .unwrap();
    assert_eq!(
        vfs.copy("a.txt", "b.txt").await.unwrap(),
        Copied::NotEligible
    );
    let items = memory.list(&memory.root()).await.unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "a.txt");
}

#[tokio::test(start_paused = true)]
async fn a_copy_is_of_the_remote_as_its_items_were_listed() {
    let (provider, vfs, _dir) = spooled_mount(Options::default(), 1024 * 1024, never_local);
    let folder = provider.add_folder(&provider.root(), "Folder");
    provider.add_file(&folder, "a.txt", b"old");
    let entry = vfs.lookup(Vfs::ROOT, "Folder").await.unwrap();
    let file = vfs.lookup(entry.node, "a.txt").await.unwrap();
    vfs.attributes(Vfs::ROOT).await.unwrap();
    // The copy checks the root and plans from what the core holds, so its first provider call makes the new folder. The edit starts while that call waits.
    provider.set_delay(Duration::from_secs(1));
    let copying = tokio::spawn({
        let vfs = Arc::clone(&vfs);
        async move { vfs.copy("Folder", "Folder copy").await }
    });
    until_in_flight(&provider, 1).await;
    let handle = vfs.open(file.node, Access::Write).await.unwrap();
    vfs.write(handle, 0, b"new").await.unwrap();
    vfs.release(handle);
    assert_eq!(
        copying.await.unwrap().unwrap(),
        Copied::To("Folder copy".into())
    );
    let copied = root_items(&provider)
        .await
        .into_iter()
        .find(|item| item.name == "Folder copy")
        .unwrap();
    let children = provider.list(&copied.id).await.unwrap();
    assert_eq!(provider.contents(&children[0].id), b"old");
}
