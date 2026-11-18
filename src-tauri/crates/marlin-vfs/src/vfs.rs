use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Semaphore;
use tokio::time::Instant;

use crate::{
    escape, unescape, unique_names, Attributes, Item, ItemId, Kind, NameRules, Provider, Space,
    VfsError,
};

/// A file or folder, for as long as the OS holds a lookup on it or a handle is open on it. The number is never reused in a mount.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u64);

/// One open file or one open folder listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HandleId(pub u64);

/// One name in a folder, as the mount shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub node: NodeId,

    /// The name after escaping and de-duplication.
    pub name: String,

    pub attributes: Attributes,
}

/// How a caller wants to open a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Read the file's bytes.
    Read,

    /// Change the file's bytes. This core rejects it.
    Write,
}

/// What a rename does when the new name is taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IfExists {
    /// The item that holds the name is removed, as POSIX `rename` does.
    Replace,

    /// The rename fails with [`VfsError::Conflict`] (Linux `RENAME_NOREPLACE`).
    Refuse,
}

/// Limits and cache times for one mount.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// How long one provider call may take, including the wait for a free slot.
    pub call_timeout: Duration,

    /// How many provider calls may run at once.
    pub max_calls: usize,

    /// How long a folder listing may be reused.
    pub listing_ttl: Duration,

    /// How many folder listings to keep. 0 turns the cache off.
    pub max_listings: usize,

    /// How long an answer about one item's attributes may be reused. Zero turns it off.
    pub attributes_ttl: Duration,

    /// How long the account's total and free space may be reused. Zero turns it off.
    pub space_ttl: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            call_timeout: Duration::from_secs(30),
            max_calls: 16,
            listing_ttl: Duration::from_secs(5),
            max_listings: 64,
            attributes_ttl: Duration::from_secs(5),
            space_ttl: Duration::from_secs(30),
        }
    }
}

/// When the core asked the provider for an answer, and when the answer came.
#[derive(Clone, Copy)]
struct Asked {
    /// When the call started. Answers are ordered by it, so a slow call's answer is never taken for a later one.
    at: Instant,

    /// `State::generation` when the call started.
    generation: u64,

    /// When the answer came. It is fresh for its time limit from then, until the mount is refreshed.
    answered: Instant,
}

impl Asked {
    fn is_fresh(self, generation: u64, ttl: Duration) -> bool {
        self.generation == generation && self.answered.elapsed() < ttl
    }
}

struct Node {
    item: ItemId,
    lookups: u64,
    handles: u64,

    /// What the provider last said about the item.
    attributes: Option<(Asked, Attributes)>,
}

enum Opened {
    File { node: NodeId },
    Folder { node: NodeId, entries: Vec<Entry> },
}

/// One folder's listing, as the provider last gave it and as the mount shows it.
struct Listing {
    asked: Asked,

    /// The items with the names the remote uses. This mount's own changes edit this list.
    remote: Vec<Item>,

    /// `remote` after `unique_names`: what a lookup and an open folder read.
    shown: Arc<Vec<Item>>,
}

struct State {
    nodes: HashMap<NodeId, Node>,
    by_item: HashMap<ItemId, NodeId>,
    next_node: u64,
    handles: HashMap<HandleId, Opened>,
    next_handle: u64,
    listings: HashMap<ItemId, Listing>,
    space: Option<(Asked, Space)>,
    generation: u64,

    /// When this mount last changed what a folder holds, or tried to. A listing the core asked for at or before that time, and that answers after it, is not kept. Entries older than the call time limit are dropped: no listing still in flight can be older.
    changed: HashMap<ItemId, Instant>,
}

/// The mount core. It can make a folder, remove an item, and rename or move an item, and it cannot write a file's bytes. Adapters share one value across tasks. The state lock is never held across a provider call. A separate lock, `changes`, is held across the provider call of a change.
pub struct Vfs {
    provider: Arc<dyn Provider>,
    rules: NameRules,
    options: Options,
    permits: Semaphore,

    /// One change runs at a time in this mount. A rename names two folders, and one lock for all changes needs no lock order.
    changes: tokio::sync::Mutex<()>,

    state: Mutex<State>,
    watcher: Option<Box<CallWatcher>>,
}

/// Hears how one provider call ended: `None` for a value, or the error.
type CallWatcher = dyn Fn(Option<&VfsError>) + Send + Sync;

impl Vfs {
    /// The node id of the folder at the top of the mount.
    pub const ROOT: NodeId = NodeId(1);

    /// Starts a mount on `provider` using these name rules and limits.
    pub fn new(provider: Arc<dyn Provider>, rules: NameRules, options: Options) -> Self {
        let root = provider.root();
        let mut nodes = HashMap::new();
        let mut by_item = HashMap::new();
        nodes.insert(
            Self::ROOT,
            Node {
                item: root.clone(),
                lookups: 0,
                handles: 0,
                attributes: None,
            },
        );
        by_item.insert(root, Self::ROOT);
        Self {
            provider,
            rules,
            permits: Semaphore::new(options.max_calls),
            options,
            changes: tokio::sync::Mutex::new(()),
            state: Mutex::new(State {
                nodes,
                by_item,
                next_node: 2,
                handles: HashMap::new(),
                next_handle: 1,
                listings: HashMap::new(),
                space: None,
                generation: 0,
                changed: HashMap::new(),
            }),
            watcher: None,
        }
    }

    /// Tells `watcher` how every provider call ended: `None` for a value, or the error. The core's own time limit reports [`VfsError::TimedOut`]. A call the adapter cancels reports nothing, and neither does a listing, an attributes answer, or a space answer the core already holds.
    pub fn watch_calls(
        mut self,
        watcher: impl Fn(Option<&VfsError>) + Send + Sync + 'static,
    ) -> Self {
        self.watcher = Some(Box::new(watcher));
        self
    }

    /// Finds `name` in `parent` and adds one lookup to the item's node.
    pub async fn lookup(&self, parent: NodeId, name: &str) -> Result<Entry, VfsError> {
        let parent_item = self.item_of(parent)?;
        let (asked, found) = self.find(&parent_item, name).await?;
        let Some(found) = found else {
            return Err(VfsError::NotFound);
        };
        let mut state = self.state.lock().unwrap();
        let node = node_for(&mut state, found.id);
        let found_node = state.nodes.get_mut(&node).unwrap();
        found_node.lookups += 1;
        let attributes = remember(&mut found_node.attributes, asked, found.attributes);
        Ok(Entry {
            node,
            name: found.name,
            attributes,
        })
    }

    /// Gives `lookups` lookups back. The node goes away when it has no lookup and no open handle left.
    pub fn forget(&self, node: NodeId, lookups: u64) {
        let mut state = self.state.lock().unwrap();
        let Some(found) = state.nodes.get_mut(&node) else {
            return;
        };
        found.lookups = found.lookups.saturating_sub(lookups);
        drop_unreferenced(&mut state, node);
    }

    /// This node's attributes: the answer the node holds while it is fresh, or a new one from the provider.
    pub async fn attributes(&self, node: NodeId) -> Result<Attributes, VfsError> {
        let (item, at, generation) = {
            let state = self.state.lock().unwrap();
            let Some(found) = state.nodes.get(&node) else {
                return Err(VfsError::StaleHandle);
            };
            if let Some((held, attributes)) = found.attributes {
                if held.is_fresh(state.generation, self.options.attributes_ttl) {
                    return Ok(attributes);
                }
            }
            (found.item.clone(), Instant::now(), state.generation)
        };
        let attributes = self.call(self.provider.item(&item)).await?.attributes;
        let asked = Asked {
            at,
            generation,
            answered: Instant::now(),
        };
        let mut state = self.state.lock().unwrap();
        let Some(found) = state.nodes.get_mut(&node) else {
            return Ok(attributes);
        };
        Ok(remember(&mut found.attributes, asked, attributes))
    }

    /// Opens a folder and freezes that listing. The folder and each item in it keep a handle reference until `release`.
    pub async fn open_folder(&self, node: NodeId) -> Result<HandleId, VfsError> {
        let item = self.item_of(node)?;
        let (asked, items) = self.listing(&item).await?;
        let mut state = self.state.lock().unwrap();
        let Some(opened) = state.nodes.get_mut(&node) else {
            return Err(VfsError::StaleHandle);
        };
        opened.handles += 1;
        let mut entries = Vec::with_capacity(items.len());
        for item in items.iter() {
            let child = node_for(&mut state, item.id.clone());
            let child_node = state.nodes.get_mut(&child).unwrap();
            child_node.handles += 1;
            let attributes = remember(&mut child_node.attributes, asked, item.attributes);
            entries.push(Entry {
                node: child,
                name: item.name.clone(),
                attributes,
            });
        }
        Ok(insert_handle(&mut state, Opened::Folder { node, entries }))
    }

    /// Returns one stored page of a folder handle. Past the end is empty, and these entries are not lookups.
    pub fn read_folder(
        &self,
        handle: HandleId,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Entry>, VfsError> {
        let state = self.state.lock().unwrap();
        let Some(Opened::Folder { entries, .. }) = state.handles.get(&handle) else {
            return Err(VfsError::StaleHandle);
        };
        let start = offset.min(entries.len());
        let end = start.saturating_add(limit).min(entries.len());
        Ok(entries[start..end].to_vec())
    }

    /// Opens a file for `access`. A folder is [`VfsError::IsAFolder`], and a write is [`VfsError::ReadOnly`].
    pub async fn open(&self, node: NodeId, access: Access) -> Result<HandleId, VfsError> {
        if access == Access::Write {
            return Err(VfsError::ReadOnly);
        }
        let attributes = self.attributes(node).await?;
        if attributes.kind == Kind::Folder {
            return Err(VfsError::IsAFolder);
        }
        let mut state = self.state.lock().unwrap();
        let Some(opened) = state.nodes.get_mut(&node) else {
            return Err(VfsError::StaleHandle);
        };
        opened.handles += 1;
        Ok(insert_handle(&mut state, Opened::File { node }))
    }

    /// Reads one byte range. The size a node holds never limits a read, so a file that grows can be read to its new end.
    pub async fn read(
        &self,
        handle: HandleId,
        offset: u64,
        length: u32,
    ) -> Result<Vec<u8>, VfsError> {
        let item = {
            let state = self.state.lock().unwrap();
            let Some(Opened::File { node }) = state.handles.get(&handle) else {
                return Err(VfsError::StaleHandle);
            };
            state.nodes.get(node).unwrap().item.clone()
        };
        self.call(self.provider.read(&item, offset, length)).await
    }

    /// Closes a handle and drops the handle references it held. An unknown handle does nothing.
    pub fn release(&self, handle: HandleId) {
        let mut state = self.state.lock().unwrap();
        let Some(opened) = state.handles.remove(&handle) else {
            return;
        };
        let nodes = match opened {
            Opened::File { node } => vec![node],
            Opened::Folder { node, entries } => std::iter::once(node)
                .chain(entries.into_iter().map(|entry| entry.node))
                .collect(),
        };
        for node in nodes {
            if let Some(found) = state.nodes.get_mut(&node) {
                found.handles -= 1;
            }
            drop_unreferenced(&mut state, node);
        }
    }

    /// How much space the account has: the answer the core holds while it is fresh, or a new one from the provider.
    pub async fn space(&self) -> Result<Space, VfsError> {
        let (at, generation) = {
            let state = self.state.lock().unwrap();
            if let Some((held, space)) = state.space {
                if held.is_fresh(state.generation, self.options.space_ttl) {
                    return Ok(space);
                }
            }
            (Instant::now(), state.generation)
        };
        let space = self.call(self.provider.space()).await?;
        let asked = Asked {
            at,
            generation,
            answered: Instant::now(),
        };
        let mut state = self.state.lock().unwrap();
        Ok(remember(&mut state.space, asked, space))
    }

    /// Drops cached listings and makes every stored attributes and space answer stale. A call already in flight cannot put a fresh result back, and a listing in flight is not kept.
    pub fn refresh(&self) {
        let mut state = self.state.lock().unwrap();
        state.listings.clear();
        state.generation += 1;
    }

    /// Makes an empty folder named `name` in `parent` and returns it. `name` is the name the OS sent. A name the mount cannot show back is [`VfsError::NotSupported`], and a name already in the listing is [`VfsError::Conflict`].
    pub async fn create_folder(&self, parent: NodeId, name: &str) -> Result<Entry, VfsError> {
        let _changes = self.lock_changes().await?;
        let parent_item = self.item_of(parent)?;
        let remote_name = self.remote_name(name)?;
        let (_asked, existing) = self.find(&parent_item, name).await?;
        if existing.is_some() {
            return Err(VfsError::Conflict);
        }
        let (at, generation) = {
            let state = self.state.lock().unwrap();
            (Instant::now(), state.generation)
        };
        let unsettled = Unsettled {
            vfs: self,
            folders: vec![parent_item.clone()],
            settled: false,
        };
        let created = self
            .call(self.provider.create_folder(&parent_item, &remote_name))
            .await?;
        let asked = Asked {
            at,
            generation,
            answered: Instant::now(),
        };
        let mut state = unsettled.confirmed();
        change_listing(&mut state, &self.rules, &parent_item, |remote| {
            remote.retain(|item| item.id != created.id);
            remote.push(created.clone());
        });
        let node = node_for(&mut state, created.id);
        let found_node = state.nodes.get_mut(&node).unwrap();
        found_node.lookups += 1;
        let attributes = remember(&mut found_node.attributes, asked, created.attributes);
        Ok(Entry {
            node,
            name: name.to_string(),
            attributes,
        })
    }

    /// Removes `name` from `parent`. `kind` is what the OS call removes: [`Kind::File`] for unlink, [`Kind::Folder`] for rmdir. The wrong kind is [`VfsError::IsAFolder`] or [`VfsError::NotAFolder`].
    pub async fn remove(&self, parent: NodeId, name: &str, kind: Kind) -> Result<(), VfsError> {
        let _changes = self.lock_changes().await?;
        let parent_item = self.item_of(parent)?;
        let (_asked, found) = self.find(&parent_item, name).await?;
        let Some(found) = found else {
            return Err(VfsError::NotFound);
        };
        if found.attributes.kind == Kind::Folder && kind == Kind::File {
            return Err(VfsError::IsAFolder);
        }
        if found.attributes.kind == Kind::File && kind == Kind::Folder {
            return Err(VfsError::NotAFolder);
        }
        let unsettled = Unsettled {
            vfs: self,
            folders: vec![parent_item.clone()],
            settled: false,
        };
        self.call(self.provider.remove(&found.id, &parent_item))
            .await?;
        let mut state = unsettled.confirmed();
        change_listing(&mut state, &self.rules, &parent_item, |remote| {
            remote.retain(|item| item.id != found.id)
        });
        Ok(())
    }

    /// Renames `name` in `parent` to `new_name` in `new_parent`. The names are the names the OS sent. `if_exists` says what to do when `new_name` is taken.
    pub async fn rename(
        &self,
        parent: NodeId,
        name: &str,
        new_parent: NodeId,
        new_name: &str,
        if_exists: IfExists,
    ) -> Result<(), VfsError> {
        let _changes = self.lock_changes().await?;
        let from = self.item_of(parent)?;
        let to = self.item_of(new_parent)?;
        let remote_name = self.remote_name(new_name)?;
        let (_asked, source) = self.find(&from, name).await?;
        let Some(source) = source else {
            return Err(VfsError::NotFound);
        };
        if from == to && new_name == source.name {
            return match if_exists {
                IfExists::Replace => Ok(()),
                IfExists::Refuse => Err(VfsError::Conflict),
            };
        }
        let (_asked, target) = self.find(&to, new_name).await?;
        let replace = match &target {
            Some(target) if target.id != source.id => {
                if if_exists == IfExists::Refuse {
                    return Err(VfsError::Conflict);
                }
                if source.attributes.kind == Kind::Folder && target.attributes.kind == Kind::File {
                    return Err(VfsError::NotAFolder);
                }
                if source.attributes.kind == Kind::File && target.attributes.kind == Kind::Folder {
                    return Err(VfsError::IsAFolder);
                }
                Some(target.id.clone())
            }
            _ => None,
        };
        let (at, generation) = {
            let state = self.state.lock().unwrap();
            (Instant::now(), state.generation)
        };
        let unsettled = Unsettled {
            vfs: self,
            folders: vec![from.clone(), to.clone()],
            settled: false,
        };
        let moved = self
            .call(
                self.provider
                    .rename(&source.id, &from, &to, &remote_name, replace.as_ref()),
            )
            .await?;
        let asked = Asked {
            at,
            generation,
            answered: Instant::now(),
        };
        let mut state = unsettled.confirmed();
        if from != to {
            change_listing(&mut state, &self.rules, &from, |remote| {
                remote.retain(|item| item.id != source.id)
            });
        }
        change_listing(&mut state, &self.rules, &to, |remote| {
            remote.retain(|item| {
                item.id != source.id && item.id != moved.id && replace.as_ref() != Some(&item.id)
            });
            remote.push(moved.clone());
        });
        if moved.id != source.id && !state.by_item.contains_key(&moved.id) {
            if let Some(node) = state.by_item.remove(&source.id) {
                state.nodes.get_mut(&node).unwrap().item = moved.id.clone();
                state.by_item.insert(moved.id.clone(), node);
            }
        }
        if let Some(node) = state.by_item.get(&moved.id).copied() {
            let found = state.nodes.get_mut(&node).unwrap();
            remember(&mut found.attributes, asked, moved.attributes);
        }
        Ok(())
    }

    fn item_of(&self, node: NodeId) -> Result<ItemId, VfsError> {
        self.state
            .lock()
            .unwrap()
            .nodes
            .get(&node)
            .map(|found| found.item.clone())
            .ok_or(VfsError::StaleHandle)
    }

    /// Runs one provider call inside the mount's limits. Dropping the returned future cancels the call.
    async fn call<T>(
        &self,
        work: impl Future<Output = Result<T, VfsError>>,
    ) -> Result<T, VfsError> {
        let limited = async {
            let _permit = self.permits.acquire().await.expect("never closed");
            work.await
        };
        let result = tokio::time::timeout(self.options.call_timeout, limited)
            .await
            .unwrap_or(Err(VfsError::TimedOut));
        if let Some(watcher) = &self.watcher {
            watcher(result.as_ref().err());
        }
        result
    }

    /// The three changes take this for the whole change. A wait that runs out is [`VfsError::TimedOut`].
    async fn lock_changes(&self) -> Result<tokio::sync::MutexGuard<'_, ()>, VfsError> {
        tokio::time::timeout(self.options.call_timeout, self.changes.lock())
            .await
            .map_err(|_| VfsError::TimedOut)
    }

    /// The remote name for a name the OS sent. A name that `escape` would not give back, such as `%41`, is [`VfsError::NotSupported`]: a lookup of the name the OS used would miss the item.
    fn remote_name(&self, name: &str) -> Result<String, VfsError> {
        let remote_name = unescape(name);
        if escape(&remote_name, &self.rules) != name {
            return Err(VfsError::NotSupported);
        }
        Ok(remote_name)
    }

    /// The item that holds `name` in `folder`, as the mount shows it. A lookup and every change resolve a name here, so they always agree on which item a name means.
    async fn find(&self, folder: &ItemId, name: &str) -> Result<(Asked, Option<Item>), VfsError> {
        let (asked, items) = self.listing(folder).await?;
        let wanted = (self.rules.key)(name);
        let found = items
            .iter()
            .find(|item| (self.rules.key)(&item.name) == wanted)
            .cloned();
        Ok((asked, found))
    }

    async fn listing(&self, folder: &ItemId) -> Result<(Asked, Arc<Vec<Item>>), VfsError> {
        let ttl = self.options.listing_ttl;
        let (at, generation) = {
            let state = self.state.lock().unwrap();
            if let Some(held) = state.listings.get(folder) {
                if held.asked.is_fresh(state.generation, ttl) {
                    return Ok((held.asked, Arc::clone(&held.shown)));
                }
            }
            (Instant::now(), state.generation)
        };
        let remote = self.call(self.provider.list(folder)).await?;
        let asked = Asked {
            at,
            generation,
            answered: Instant::now(),
        };
        let shown = shown(&remote, &self.rules);
        let mut state = self.state.lock().unwrap();
        let later = state
            .listings
            .get(folder)
            .is_some_and(|held| held.asked.at > at);
        let changed = state
            .changed
            .get(folder)
            .is_some_and(|changed| *changed >= at);
        if state.generation != generation || later || changed {
            return Ok((asked, shown));
        }
        if state.listings.len() >= self.options.max_listings {
            state
                .listings
                .retain(|_, held| held.asked.answered.elapsed() < ttl);
        }
        if state.listings.len() >= self.options.max_listings {
            let oldest = state
                .listings
                .iter()
                .min_by_key(|(_, held)| held.asked.answered);
            if let Some(oldest) = oldest.map(|(id, _)| id.clone()) {
                state.listings.remove(&oldest);
            }
        }
        if state.listings.len() < self.options.max_listings {
            state.listings.insert(
                folder.clone(),
                Listing {
                    asked,
                    remote,
                    shown: Arc::clone(&shown),
                },
            );
        }
        Ok((asked, shown))
    }
}

/// A change the provider was asked to make. It ends one of two ways: `confirmed`, or dropped. Dropped (an error, the time limit, or a cancelled call), the remote may or may not have changed, so the cached listing of each folder it names is forgotten.
struct Unsettled<'a> {
    vfs: &'a Vfs,
    folders: Vec<ItemId>,
    settled: bool,
}

impl<'a> Unsettled<'a> {
    /// The provider confirmed the change. Stamps each folder and returns the state for the listing edits.
    fn confirmed(mut self) -> std::sync::MutexGuard<'a, State> {
        self.settled = true;
        let mut state = self.vfs.state.lock().unwrap();
        for folder in &self.folders {
            note_changed(&mut state, self.vfs.options.call_timeout, folder);
        }
        state
    }
}

impl Drop for Unsettled<'_> {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        let mut state = self.vfs.state.lock().unwrap();
        for folder in &self.folders {
            note_changed(&mut state, self.vfs.options.call_timeout, folder);
            state.listings.remove(folder);
        }
    }
}

/// A listing as the mount shows it: the remote's items after `unique_names`.
fn shown(remote: &[Item], rules: &NameRules) -> Arc<Vec<Item>> {
    let mut shown = remote.to_vec();
    unique_names(&mut shown, rules);
    Arc::new(shown)
}

/// Stamps `State::changed` for `folder` and drops the stamps no listing in flight can be older than.
fn note_changed(state: &mut State, call_timeout: Duration, folder: &ItemId) {
    let now = Instant::now();
    state
        .changed
        .retain(|_, at| now.saturating_duration_since(*at) < call_timeout);
    state.changed.insert(folder.clone(), now);
}

/// Edits the cached listing of `folder`, if one is held, so this mount's own change shows without asking the provider.
fn change_listing(
    state: &mut State,
    rules: &NameRules,
    folder: &ItemId,
    edit: impl FnOnce(&mut Vec<Item>),
) {
    let Some(listing) = state.listings.get_mut(folder) else {
        return;
    };
    edit(&mut listing.remote);
    listing.shown = shown(&listing.remote, rules);
}

fn node_for(state: &mut State, item: ItemId) -> NodeId {
    if let Some(id) = state.by_item.get(&item).copied() {
        return id;
    }
    let id = NodeId(state.next_node);
    state.next_node += 1;
    state.by_item.insert(item.clone(), id);
    state.nodes.insert(
        id,
        Node {
            item,
            lookups: 0,
            handles: 0,
            attributes: None,
        },
    );
    id
}

/// Stores an answer from the provider and returns the latest answer held. An answer asked for earlier never replaces one asked for later: a lookup can carry a listing that is older than the node's own answer, and a slow call can end after a later one.
fn remember<T: Copy>(held: &mut Option<(Asked, T)>, asked: Asked, answer: T) -> T {
    match *held {
        Some((kept, latest)) if (kept.generation, kept.at) > (asked.generation, asked.at) => latest,
        _ => {
            *held = Some((asked, answer));
            answer
        }
    }
}

fn drop_unreferenced(state: &mut State, node: NodeId) {
    if node == Vfs::ROOT {
        return;
    }
    let Some(found) = state.nodes.get(&node) else {
        return;
    };
    if found.lookups != 0 || found.handles != 0 {
        return;
    }
    let item = found.item.clone();
    state.nodes.remove(&node);
    state.by_item.remove(&item);
}

fn insert_handle(state: &mut State, opened: Opened) -> HandleId {
    let handle = HandleId(state.next_handle);
    state.next_handle += 1;
    state.handles.insert(handle, opened);
    handle
}
