use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use marlin_vfs::{Access, Kind, MemoryProvider, Options, Provider, Vfs, VfsError, POSIX};

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
    provider.rename(&file, &folder_b, "moved.txt");
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
