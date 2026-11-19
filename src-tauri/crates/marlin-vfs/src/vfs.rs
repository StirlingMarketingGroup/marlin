use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use tokio::sync::{Notify, Semaphore};
use tokio::time::Instant;

use crate::spool::{self, Spool};
use crate::{
    escape, unescape, unique_names, Attributes, Item, ItemId, Kind, NameRules, Provider, Revision,
    Space, VfsError,
};

mod upload;

use upload::Attempt;
pub use upload::{Unsent, UnsentState};

/// A file or folder. It lives while the OS holds a lookup on it or a handle is open on it, and while it owns a spool file as an item or a new file. The number is never reused in a mount.
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

    /// Change the file's bytes. A mount with no spool rejects it.
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
    /// How long one provider call may take, including the wait for a free slot. A name change waits this long for its turn, and a write waits this long for room in the spool.
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

    /// How long a file waits after its last writer closes, or after its last change, before it uploads.
    pub settle: Duration,

    /// How many uploads may run at once. An upload's byte calls never take a slot of [`Self::max_calls`] and have no [`Self::call_timeout`]. A conflict copy's `item` call and the remove of a stray item are ordinary calls.
    pub max_uploads: usize,

    /// An upload whose progress has not moved for this long is stopped and fails with [`VfsError::TimedOut`].
    pub upload_stall: Duration,
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
            settle: Duration::from_secs(2),
            max_uploads: 4,
            upload_stall: Duration::from_secs(60),
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

/// A folder, and a remote name in it.
#[derive(Clone)]
struct Place {
    folder: ItemId,

    /// `unescape` of the name the mount shows, so a duplicate shown as `a (2).txt` is `a (2).txt` here.
    name: String,
}

/// What a node is.
enum Identity {
    /// An item the provider has. `place` is where this mount last showed it: `None` only for the root.
    Item { id: ItemId, place: Option<Place> },

    /// An item this mount removed, or replaced by a rename. Calls on it get what the provider answers for the id, and its bytes never go anywhere.
    Removed(ItemId),

    /// A file that is only in the spool.
    New(Place),

    /// A new file that was removed, or replaced by a rename. It has no name. Its open handles still work.
    Unnamed,
}

impl Identity {
    /// The id the provider knows this node by. A file that is only in the spool has none.
    fn remote_id(&self) -> Option<&ItemId> {
        match self {
            Identity::Item { id, .. } | Identity::Removed(id) => Some(id),
            Identity::New(_) | Identity::Unnamed => None,
        }
    }
}

/// Where a file's bytes are. Decision 0005 calls `Clean` "Clean"; every other state in its table is `Spooled`.
#[derive(Clone)]
enum Bytes {
    /// The remote has them. Reads ask the provider.
    Clean,

    /// A first write or a truncate is making the spool file. Reads still ask the provider. The fetch holds the lock; another write or truncate waits on it, then looks again.
    Fetching(Arc<tokio::sync::Mutex<()>>),

    /// The spool file is the file: every read, the size, and the modified time come from it.
    Spooled(SpoolId),
}

/// One spool file, by the number that names it in the spool folder.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct SpoolId(u64);

/// One file in the spool folder.
struct SpoolFile {
    path: PathBuf,

    /// The file's length on the disk, which the mount shows.
    size: u64,

    /// Room that the edit in flight counted beyond `size`. [`State::spool_used`] counts both.
    reserved: u64,

    modified: SystemTime,

    /// Held by each write and truncate from its room check to the end of its disk call, so two edits never interleave their size and room accounting, whichever handle they come through.
    edits: Arc<tokio::sync::Mutex<()>>,

    /// The node whose bytes this file holds.
    node: NodeId,

    /// The revision of the remote bytes this file was based on. `None` for a new file, and for an item whose provider gave no revision.
    base: Option<Revision>,

    /// Goes up on every change to the bytes or the modified time. A confirmed upload deletes the file only when this is the value the try started with.
    version: u64,

    /// True from an edit's room reservation until its disk call has been accounted. Edits of one file already run one at a time.
    editing: bool,

    /// When the next try may start.
    due: Instant,

    /// The try that is running, if one is.
    attempt: Option<Attempt>,

    /// The last failed try's error, and how many tries in a row have failed.
    failure: Option<(VfsError, u32)>,
}

struct Node {
    identity: Identity,
    lookups: u64,
    handles: u64,

    /// Handles open on this node with [`Access::Write`].
    writers: u64,

    /// What the provider last said about the item.
    attributes: Option<(Asked, Attributes)>,

    bytes: Bytes,
}

enum Opened {
    File { node: NodeId, access: Access },
    Folder { node: NodeId, entries: Vec<Entry> },
}

/// What holds one name in a folder, as the mount shows it.
enum Holder {
    /// An item in the provider's listing, with its shown name.
    Item(Item),

    /// A file that is only in the spool, and its shown name (`escape` of its remote name).
    New { node: NodeId, name: String },
}

impl Holder {
    fn name(&self) -> &str {
        match self {
            Holder::Item(item) => &item.name,
            Holder::New { name, .. } => name,
        }
    }

    fn kind(&self) -> Kind {
        match self {
            Holder::Item(item) => item.attributes.kind,
            Holder::New { .. } => Kind::File,
        }
    }
}

/// One folder's listing, as the provider last gave it and as the mount shows it.
#[derive(Clone)]
struct Listing {
    asked: Asked,

    /// The items with the names the remote uses. This mount's own changes edit this list.
    remote: Arc<Vec<Item>>,

    /// `remote` after `unique_names`: what a lookup and an open folder read, before new files are laid over it.
    shown: Arc<Vec<Item>>,

    /// Where each name in `shown` is, by its key, so a lookup does not walk the folder.
    index: Arc<HashMap<String, usize>>,
}

impl Listing {
    /// A listing of `remote`, with the names the mount shows and the index that finds them.
    fn new(asked: Asked, remote: Arc<Vec<Item>>, rules: &NameRules) -> Self {
        let mut shown = remote.to_vec();
        unique_names(&mut shown, rules, &[]);
        let index = shown
            .iter()
            .enumerate()
            .map(|(at, item)| ((rules.key)(&item.name), at))
            .collect();
        Self {
            asked,
            remote,
            shown: Arc::new(shown),
            index: Arc::new(index),
        }
    }
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

    spools: HashMap<SpoolId, SpoolFile>,

    /// For each folder, its new files by the key of the name they show under. A folder with none has no entry.
    new_files: HashMap<ItemId, HashMap<String, NodeId>>,

    /// The sum of `size` and `reserved` over every [`SpoolFile`].
    spool_used: u64,

    /// When a spool file may next be sent. An entry stays after the file changes. The uploader drops one that cannot start.
    queue: BTreeSet<(Instant, SpoolId)>,
}

/// How much of a remote file one fetch reads at a time. A stored size never limits a read, so the piece size is the only cap.
const PIECE: u32 = 4 * 1024 * 1024;

/// The mount core. With a spool it can create, write, cut, rename, and remove a file locally, and [`Vfs::run_uploads`] sends those files. Adapters share one value across tasks. The state lock is never held across a provider call. The only spool file call made under it is the delete of a spool file. A separate lock, `changes`, is held across the provider call of a name change, and across the confirm of an upload.
pub struct Vfs {
    provider: Arc<dyn Provider>,
    rules: NameRules,
    options: Options,
    permits: Semaphore,

    /// One change runs at a time in this mount. A rename names two folders, and one lock for all changes needs no lock order.
    changes: tokio::sync::Mutex<()>,

    /// Shared with a disk call, which accounts for the edit before it lets the next one in.
    state: Arc<Mutex<State>>,

    /// Wakes a write that is waiting for the spool to shrink.
    room: Arc<Notify>,

    /// Wakes the uploader. Shared so a disk call can wake it after it drops the state lock. `notify_one` keeps a wake that arrives while the uploader is busy.
    uploads: Arc<Notify>,

    spool: Option<Spool>,

    /// The next spool file's number, which is also its name in the spool folder. A file already there keeps its number.
    next_spool: std::sync::atomic::AtomicU64,

    watcher: Option<Box<CallWatcher>>,
}

/// Hears how one provider call ended: `None` for a value, or the error.
type CallWatcher = dyn Fn(Option<&VfsError>) + Send + Sync;

impl Vfs {
    /// The node id of the folder at the top of the mount.
    pub const ROOT: NodeId = NodeId(1);

    /// Starts a mount on `provider` using these name rules and limits. It takes no writes until [`Self::take_writes`].
    pub fn new(provider: Arc<dyn Provider>, rules: NameRules, options: Options) -> Self {
        let root = provider.root();
        let mut nodes = HashMap::new();
        let mut by_item = HashMap::new();
        nodes.insert(
            Self::ROOT,
            Node {
                identity: Identity::Item {
                    id: root.clone(),
                    place: None,
                },
                lookups: 0,
                handles: 0,
                writers: 0,
                attributes: None,
                bytes: Bytes::Clean,
            },
        );
        by_item.insert(root, Self::ROOT);
        Self {
            provider,
            rules,
            permits: Semaphore::new(options.max_calls),
            options,
            changes: tokio::sync::Mutex::new(()),
            state: Arc::new(Mutex::new(State {
                nodes,
                by_item,
                next_node: 2,
                handles: HashMap::new(),
                next_handle: 1,
                listings: HashMap::new(),
                space: None,
                generation: 0,
                changed: HashMap::new(),
                spools: HashMap::new(),
                new_files: HashMap::new(),
                spool_used: 0,
                queue: BTreeSet::new(),
            })),
            room: Arc::new(Notify::new()),
            uploads: Arc::new(Notify::new()),
            spool: None,
            next_spool: std::sync::atomic::AtomicU64::new(1),
            watcher: None,
        }
    }

    /// Tells `watcher` how every provider call ended: `None` for a value, or the error. The core's own time limit reports [`VfsError::TimedOut`]. A call the adapter cancels reports nothing, and neither does a listing, an attributes answer, or a space answer the core already holds. An upload's byte call reports like any other call, and an upload the core stops reports nothing.
    pub fn watch_calls(
        mut self,
        watcher: impl Fn(Option<&VfsError>) + Send + Sync + 'static,
    ) -> Self {
        self.watcher = Some(Box::new(watcher));
        self
    }

    /// The mount's spool. A mount with none takes no writes: [`VfsError::ReadOnly`].
    fn spool(&self) -> Result<&Spool, VfsError> {
        self.spool.as_ref().ok_or(VfsError::ReadOnly)
    }

    /// Lets the mount take writes into `spool`. Without one, opening a file for writing, [`Self::create`], [`Self::truncate`], and [`Self::set_modified`] are [`VfsError::ReadOnly`].
    pub fn take_writes(mut self, spool: Spool) -> Self {
        self.spool = Some(spool);
        self
    }

    /// Finds `name` in `parent` and adds one lookup to the item's node.
    pub async fn lookup(&self, parent: NodeId, name: &str) -> Result<Entry, VfsError> {
        let parent_item = self.folder_of(parent)?;
        let listing = self.listing(&parent_item).await?;
        let mut state = self.state.lock().unwrap();
        // The holder and the lookup share this lock, so a remove cannot drop a new file between them.
        let Some(holder) = resolve(&state, &self.rules, &parent_item, &listing, name) else {
            return Err(VfsError::NotFound);
        };
        let entry = entry_for(&mut state, &self.rules, &parent_item, &listing, holder);
        state.nodes.get_mut(&entry.node).unwrap().lookups += 1;
        Ok(entry)
    }

    /// Gives `lookups` lookups back. The node goes away when nothing holds it: no lookup, no open handle, and no spool file it owns as an item or a new file.
    pub fn forget(&self, node: NodeId, lookups: u64) {
        let mut state = self.state.lock().unwrap();
        let Some(found) = state.nodes.get_mut(&node) else {
            return;
        };
        found.lookups = found.lookups.saturating_sub(lookups);
        drop_unreferenced(&mut state, &self.room, node);
    }

    /// This node's attributes. A spooled file answers from the spool. Anything else is the answer the node holds while it is fresh, or a new one from the provider.
    pub async fn attributes(&self, node: NodeId) -> Result<Attributes, VfsError> {
        let (item, at, generation) = {
            let state = self.state.lock().unwrap();
            let Some(found) = state.nodes.get(&node) else {
                return Err(VfsError::StaleHandle);
            };
            if let Some(attributes) = spool_attributes(&state, node) {
                return Ok(attributes);
            }
            if let Some((held, attributes)) = found.attributes.clone() {
                if held.is_fresh(state.generation, self.options.attributes_ttl) {
                    return Ok(attributes);
                }
            }
            let Some(item) = found.identity.remote_id() else {
                return Err(VfsError::StaleHandle);
            };
            (item.clone(), Instant::now(), state.generation)
        };
        let attributes = self.call(self.provider.item(&item)).await?.attributes;
        let asked = Asked {
            at,
            generation,
            answered: Instant::now(),
        };
        let mut state = self.state.lock().unwrap();
        if let Some(spooled) = spool_attributes(&state, node) {
            return Ok(spooled);
        }
        let Some(found) = state.nodes.get_mut(&node) else {
            return Ok(attributes);
        };
        Ok(remember(&mut found.attributes, asked, attributes))
    }

    /// Opens a folder and freezes that listing. The folder and each item in it keep a handle reference until `release`. New files are part of the frozen copy.
    pub async fn open_folder(&self, node: NodeId) -> Result<HandleId, VfsError> {
        let item = self.folder_of(node)?;
        let listing = self.listing(&item).await?;
        let mut state = self.state.lock().unwrap();
        let Some(opened) = state.nodes.get_mut(&node) else {
            return Err(VfsError::StaleHandle);
        };
        opened.handles += 1;
        let items = renamed_items(&state, &self.rules, &item, &listing)
            .map_or_else(|| Arc::clone(&listing.shown), Arc::new);
        let new_files: Vec<Holder> = state.new_files.get(&item).map_or(Vec::new(), |held| {
            held.values()
                .map(|node| Holder::New {
                    node: *node,
                    name: new_file_name(&state, &self.rules, *node),
                })
                .collect()
        });
        let mut entries = Vec::with_capacity(items.len() + new_files.len());
        let holders = items.iter().cloned().map(Holder::Item).chain(new_files);
        for holder in holders {
            let entry = entry_for(&mut state, &self.rules, &item, &listing, holder);
            state.nodes.get_mut(&entry.node).unwrap().handles += 1;
            entries.push(entry);
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

    /// Opens a file for `access`. A folder is [`VfsError::IsAFolder`]. A write on a mount with no spool is [`VfsError::ReadOnly`]. Opening for write asks the provider nothing: it decides the kind from what the node holds.
    pub async fn open(&self, node: NodeId, access: Access) -> Result<HandleId, VfsError> {
        if access == Access::Write {
            self.check_writable(node)?;
        } else {
            let attributes = self.attributes(node).await?;
            if attributes.kind == Kind::Folder {
                return Err(VfsError::IsAFolder);
            }
        }
        let mut state = self.state.lock().unwrap();
        let Some(opened) = state.nodes.get_mut(&node) else {
            return Err(VfsError::StaleHandle);
        };
        opened.handles += 1;
        if access == Access::Write {
            opened.writers += 1;
        }
        Ok(insert_handle(&mut state, Opened::File { node, access }))
    }

    /// Reads one byte range. A spooled file is read from the spool, for every handle on it. The size a node holds never limits a read, so a file that grows can be read to its new end.
    pub async fn read(
        &self,
        handle: HandleId,
        offset: u64,
        length: u32,
    ) -> Result<Vec<u8>, VfsError> {
        loop {
            let source = {
                let state = self.state.lock().unwrap();
                let Some(Opened::File { node, .. }) = state.handles.get(&handle) else {
                    return Err(VfsError::StaleHandle);
                };
                let Some(found) = state.nodes.get(node) else {
                    return Err(VfsError::StaleHandle);
                };
                match &found.bytes {
                    Bytes::Spooled(id) => ReadSource::Spool {
                        id: *id,
                        path: state.spools[id].path.clone(),
                    },
                    Bytes::Clean | Bytes::Fetching(_) => match found.identity.remote_id() {
                        Some(id) => ReadSource::Remote(id.clone()),
                        None => return Err(VfsError::StaleHandle),
                    },
                }
            };
            match source {
                ReadSource::Spool { id, path } => {
                    let read =
                        tokio::task::spawn_blocking(move || spool::read_at(&path, offset, length))
                            .await;
                    match read {
                        Ok(Ok(bytes)) => return Ok(bytes),
                        // A confirmed upload deleted the spool file after this read took its path, so the next pass reads from where the node's bytes are now.
                        Ok(Err(error))
                            if error.kind() == std::io::ErrorKind::NotFound
                                && !upload::node_holds(&self.state.lock().unwrap(), id) => {}
                        Ok(Err(error)) => return Err(spool::map_io(error)),
                        Err(error) => return Err(VfsError::Other(error.to_string())),
                    }
                }
                ReadSource::Remote(id) => {
                    return self.call(self.provider.read(&id, offset, length)).await
                }
            }
        }
    }

    /// Writes `data` at `offset`. A handle that is not open for write is [`VfsError::AccessDenied`]: a mount with no spool has none. An empty write changes nothing.
    pub async fn write(&self, handle: HandleId, offset: u64, data: &[u8]) -> Result<(), VfsError> {
        let node = {
            let state = self.state.lock().unwrap();
            match state.handles.get(&handle) {
                Some(Opened::File { node, access }) => {
                    if *access != Access::Write {
                        return Err(VfsError::AccessDenied);
                    }
                    *node
                }
                _ => return Err(VfsError::StaleHandle),
            }
        };
        if data.is_empty() {
            return Ok(());
        }
        let data = Arc::new(data.to_vec());
        self.edit(node, Edit::Write { offset, data }).await
    }

    /// Cuts or grows a file. A mount with no spool answers [`VfsError::ReadOnly`], and a folder is [`VfsError::IsAFolder`]. Cutting a clean file to zero reads nothing from the provider.
    pub async fn truncate(&self, node: NodeId, size: u64) -> Result<Attributes, VfsError> {
        self.check_writable(node)?;
        self.edit(node, Edit::SetLen(size)).await?;
        let state = self.state.lock().unwrap();
        spool_attributes(&state, node).ok_or(VfsError::StaleHandle)
    }

    /// Sets the modified time of a spooled file. The upload sends that time with the file's bytes.
    ///
    /// A clean or fetching file, and a folder, are left as they are: no provider call sets only a time, and fetching a whole file to send a time is not worth it. A mount with no spool is [`VfsError::ReadOnly`]. A node that is gone is [`VfsError::StaleHandle`].
    pub async fn set_modified(
        &self,
        node: NodeId,
        modified: SystemTime,
    ) -> Result<Attributes, VfsError> {
        self.spool()?;
        // The lock's scope ends before the await, so an adapter can spawn this call.
        let spooled = {
            let mut state = self.state.lock().unwrap();
            let Some(found) = state.nodes.get(&node) else {
                return Err(VfsError::StaleHandle);
            };
            match found.bytes {
                Bytes::Spooled(id) => {
                    state.spools.get_mut(&id).unwrap().modified = modified;
                    upload::bytes_changed(&mut state, id);
                    upload::schedule(
                        &mut state,
                        &self.uploads,
                        id,
                        Instant::now() + self.options.settle,
                    );
                    spool_attributes(&state, node)
                }
                _ => None,
            }
        };
        match spooled {
            Some(attributes) => Ok(attributes),
            None => self.attributes(node).await,
        }
    }

    /// Makes a new empty file named `name` in `parent`, opens it, and counts one lookup and one handle. `name` is the name the OS sent. A mount with no spool answers [`VfsError::ReadOnly`], and a name that is taken is [`VfsError::Conflict`].
    pub async fn create(
        &self,
        parent: NodeId,
        name: &str,
        access: Access,
    ) -> Result<(Entry, HandleId), VfsError> {
        self.spool()?;
        let _changes = self.lock_changes().await?;
        let parent_item = self.folder_of(parent)?;
        let (_, existing) = self.find(&parent_item, name).await?;
        if existing.is_some() {
            return Err(VfsError::Conflict);
        }
        // The spool file names its node, so the number comes first.
        let node = {
            let mut state = self.state.lock().unwrap();
            state.next_node += 1;
            NodeId(state.next_node - 1)
        };
        let spool_id = self.create_spool_file(node, None).await?;
        let mut state = self.state.lock().unwrap();
        state.nodes.insert(
            node,
            Node {
                // `list` names it below, with the index that finds it.
                identity: Identity::Unnamed,
                lookups: 1,
                handles: 1,
                writers: u64::from(access == Access::Write),
                attributes: None,
                bytes: Bytes::Spooled(spool_id),
            },
        );
        list(&mut state, &self.rules, node, &parent_item, name);
        // A writer holds the upload back. A file made for reading uploads empty, as `touch` needs.
        upload::schedule(
            &mut state,
            &self.uploads,
            spool_id,
            Instant::now() + self.options.settle,
        );
        let attributes = spool_attributes(&state, node).unwrap();
        let handle = insert_handle(&mut state, Opened::File { node, access });
        Ok((
            Entry {
                node,
                name: name.to_string(),
                attributes,
            },
            handle,
        ))
    }

    /// Closes a handle and drops the handle references it held. An unknown handle does nothing.
    pub fn release(&self, handle: HandleId) {
        let mut state = self.state.lock().unwrap();
        let Some(opened) = state.handles.remove(&handle) else {
            return;
        };
        // The last writer's close starts the settle time.
        if let Opened::File {
            node,
            access: Access::Write,
        } = &opened
        {
            let found = state.nodes.get_mut(node).unwrap();
            found.writers -= 1;
            if let (0, Bytes::Spooled(id)) = (found.writers, &found.bytes) {
                let id = *id;
                upload::schedule(
                    &mut state,
                    &self.uploads,
                    id,
                    Instant::now() + self.options.settle,
                );
            }
        }
        let nodes = match opened {
            Opened::File { node, .. } => vec![node],
            Opened::Folder { node, entries } => std::iter::once(node)
                .chain(entries.into_iter().map(|entry| entry.node))
                .collect(),
        };
        for node in nodes {
            if let Some(found) = state.nodes.get_mut(&node) {
                found.handles -= 1;
            }
            drop_unreferenced(&mut state, &self.room, node);
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

    /// Makes an empty folder named `name` in `parent` and returns it. `name` is the name the OS sent. A name already in the listing, including a new file, is [`VfsError::Conflict`].
    pub async fn create_folder(&self, parent: NodeId, name: &str) -> Result<Entry, VfsError> {
        let _changes = self.lock_changes().await?;
        let parent_item = self.folder_of(parent)?;
        let remote_name = unescape(name, &self.rules);
        let (_, existing) = self.find(&parent_item, name).await?;
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
        set_place(found_node, &parent_item, remote_name);
        let attributes = remember(&mut found_node.attributes, asked, created.attributes);
        Ok(Entry {
            node,
            name: name.to_string(),
            attributes,
        })
    }

    /// Removes `name` from `parent`. `kind` is what the OS call removes: [`Kind::File`] for unlink, [`Kind::Folder`] for rmdir. The wrong kind is [`VfsError::IsAFolder`] or [`VfsError::NotAFolder`]. A new file is found in the folder listing and removed from the spool, with no change call. A folder that holds a new file is [`VfsError::NotEmpty`].
    pub async fn remove(&self, parent: NodeId, name: &str, kind: Kind) -> Result<(), VfsError> {
        let _changes = self.lock_changes().await?;
        let parent_item = self.folder_of(parent)?;
        let (_, found) = self.find(&parent_item, name).await?;
        let Some(found) = found else {
            return Err(VfsError::NotFound);
        };
        if found.kind() == Kind::Folder && kind == Kind::File {
            return Err(VfsError::IsAFolder);
        }
        if found.kind() == Kind::File && kind == Kind::Folder {
            return Err(VfsError::NotAFolder);
        }
        let item = match found {
            Holder::New { node, .. } => {
                unname(
                    &mut self.state.lock().unwrap(),
                    &self.rules,
                    &self.room,
                    node,
                );
                return Ok(());
            }
            Holder::Item(item) => item,
        };
        {
            let mut state = self.state.lock().unwrap();
            // The remote does not know the new files in a folder, so it would remove the folder and lose them.
            if state.new_files.contains_key(&item.id) {
                return Err(VfsError::NotEmpty);
            }
            // A replace that answered after the remove would find the item gone and make it again. The file is not tried again while the remove can still be running: the call time limit bounds it.
            if let Some(spool_id) =
                state
                    .by_item
                    .get(&item.id)
                    .and_then(|node| match state.nodes[node].bytes {
                        Bytes::Spooled(spool_id) => Some(spool_id),
                        _ => None,
                    })
            {
                upload::stop_attempt(&mut state, spool_id);
                upload::schedule(
                    &mut state,
                    &self.uploads,
                    spool_id,
                    Instant::now() + self.options.call_timeout,
                );
            }
        }
        let unsettled = Unsettled {
            vfs: self,
            folders: vec![parent_item.clone()],
            settled: false,
        };
        self.call(self.provider.remove(&item.id, &parent_item))
            .await?;
        let mut state = unsettled.confirmed();
        change_listing(&mut state, &self.rules, &parent_item, |remote| {
            remote.retain(|listed| listed.id != item.id)
        });
        mark_removed(&mut state, &self.room, item.id);
        Ok(())
    }

    /// Renames `name` in `parent` to `new_name` in `new_parent`. The names are the names the OS sent, and the provider gets `new_name` unescaped: moving a duplicate shown as `a (2).txt` names the remote item `a (2).txt`. `if_exists` says what to do when `new_name` is taken. A rename of an item onto a name that another item in the destination also holds on the remote is [`VfsError::Conflict`]. A new file renamed over an item keeps its own node and bytes and takes that item's id. Replacing a folder that holds a new file is [`VfsError::NotEmpty`].
    pub async fn rename(
        &self,
        parent: NodeId,
        name: &str,
        new_parent: NodeId,
        new_name: &str,
        if_exists: IfExists,
    ) -> Result<(), VfsError> {
        let _changes = self.lock_changes().await?;
        let from = self.folder_of(parent)?;
        let to = self.folder_of(new_parent)?;
        let (_, source) = self.find(&from, name).await?;
        let Some(source) = source else {
            return Err(VfsError::NotFound);
        };
        if from == to && new_name == source.name() {
            return match if_exists {
                IfExists::Replace => Ok(()),
                IfExists::Refuse => Err(VfsError::Conflict),
            };
        }
        let (destination, target) = self.find(&to, new_name).await?;
        // A holder that is the source itself, as in a case-only rename under a key that folds case, is no target.
        let target = target.filter(|target| match (&source, target) {
            (Holder::New { node: left, .. }, Holder::New { node: right, .. }) => left != right,
            (Holder::Item(left), Holder::Item(right)) => left.id != right.id,
            _ => true,
        });
        if let Some(target) = &target {
            if if_exists == IfExists::Refuse {
                return Err(VfsError::Conflict);
            }
            if source.kind() == Kind::Folder && target.kind() == Kind::File {
                return Err(VfsError::NotAFolder);
            }
            if source.kind() == Kind::File && target.kind() == Kind::Folder {
                return Err(VfsError::IsAFolder);
            }
        }
        let source = match source {
            Holder::New { node, .. } => {
                self.rename_new(node, &to, new_name, target, destination.asked);
                return Ok(());
            }
            Holder::Item(item) => item,
        };
        let remote_name = unescape(new_name, &self.rules);
        let replace = match &target {
            Some(Holder::Item(item)) => Some(item.id.clone()),
            _ => None,
        };
        // The remote does not know the new files in a folder, so it would replace the folder and lose them.
        if let Some(replace) = &replace {
            if self.state.lock().unwrap().new_files.contains_key(replace) {
                return Err(VfsError::NotEmpty);
            }
        }
        // Google Drive can hold one name twice in a folder, and `unique_names` gives the name to the one with the lower ID. With another holder left, the moved item would get `new_name` only by the luck of its ID.
        let new_key = (self.rules.key)(new_name);
        let held_by_another = destination.remote.iter().any(|item| {
            item.id != source.id
                && replace.as_ref() != Some(&item.id)
                && (self.rules.key)(&escape(&item.name, &self.rules)) == new_key
        });
        if held_by_another {
            return Err(VfsError::Conflict);
        }
        // A second owner of the cached list would make `change_listing` copy it under the state lock.
        drop(destination);
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
        // The replaced item goes first: a provider whose ids are paths gives the moved item the replaced item's id.
        match target {
            Some(Holder::Item(item)) => mark_removed(&mut state, &self.room, item.id),
            Some(Holder::New { node, .. }) => unname(&mut state, &self.rules, &self.room, node),
            None => {}
        }
        if moved.id != source.id && !state.by_item.contains_key(&moved.id) {
            if let Some(node) = state.by_item.remove(&source.id) {
                if let Identity::Item { id, .. } = &mut state.nodes.get_mut(&node).unwrap().identity
                {
                    *id = moved.id.clone();
                }
                state.by_item.insert(moved.id.clone(), node);
            }
            // The new files in a moved folder follow it to its new id.
            if let Some(children) = state.new_files.remove(&source.id) {
                for child in children.values() {
                    if let Identity::New(place) = &mut state.nodes.get_mut(child).unwrap().identity
                    {
                        place.folder = moved.id.clone();
                    }
                }
                state.new_files.insert(moved.id.clone(), children);
            }
        }
        if let Some(node) = state.by_item.get(&moved.id).copied() {
            let found = state.nodes.get_mut(&node).unwrap();
            remember(&mut found.attributes, asked, moved.attributes);
            set_place(found, &to, remote_name);
            // The new name can move the file into or out of the names that stay local, so the uploader looks at it again.
            if let Bytes::Spooled(spool_id) = found.bytes {
                upload::schedule(
                    &mut state,
                    &self.uploads,
                    spool_id,
                    Instant::now() + self.options.settle,
                );
            }
        }
        Ok(())
    }

    /// The folder's item id. A new file is not a folder. A node that is gone is [`VfsError::StaleHandle`].
    fn folder_of(&self, node: NodeId) -> Result<ItemId, VfsError> {
        let state = self.state.lock().unwrap();
        let Some(found) = state.nodes.get(&node) else {
            return Err(VfsError::StaleHandle);
        };
        found
            .identity
            .remote_id()
            .cloned()
            .ok_or(VfsError::NotAFolder)
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

    /// `create`, `create_folder`, `remove`, `rename`, and `discard_unsent` take this for the whole change. A wait that runs out is [`VfsError::TimedOut`].
    async fn lock_changes(&self) -> Result<tokio::sync::MutexGuard<'_, ()>, VfsError> {
        tokio::time::timeout(self.options.call_timeout, self.changes.lock())
            .await
            .map_err(|_| VfsError::TimedOut)
    }

    /// The holder of `name` in `folder`, as the mount shows it, and the listing it was found in. A lookup and every change resolve a name here, so they always agree on which item a name means.
    async fn find(
        &self,
        folder: &ItemId,
        name: &str,
    ) -> Result<(Listing, Option<Holder>), VfsError> {
        let listing = self.listing(folder).await?;
        let state = self.state.lock().unwrap();
        let holder = resolve(&state, &self.rules, folder, &listing, name);
        Ok((listing, holder))
    }

    async fn listing(&self, folder: &ItemId) -> Result<Listing, VfsError> {
        let ttl = self.options.listing_ttl;
        let (at, generation) = {
            let state = self.state.lock().unwrap();
            if let Some(held) = state.listings.get(folder) {
                if held.asked.is_fresh(state.generation, ttl) {
                    return Ok(held.clone());
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
        let listing = Listing::new(asked, Arc::new(remote), &self.rules);
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
            return Ok(listing);
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
            state.listings.insert(folder.clone(), listing.clone());
        }
        Ok(listing)
    }

    /// Whether `node` may be opened for write or cut: the mount has a spool, and the node is a file. A spooled node is a file, and a new file is always spooled. An item uses the kind it already holds, however old that answer is. A node with no answer is [`VfsError::StaleHandle`]: this asks the provider nothing.
    fn check_writable(&self, node: NodeId) -> Result<(), VfsError> {
        self.spool()?;
        if node == Self::ROOT {
            return Err(VfsError::IsAFolder);
        }
        let state = self.state.lock().unwrap();
        let Some(found) = state.nodes.get(&node) else {
            return Err(VfsError::StaleHandle);
        };
        if matches!(found.bytes, Bytes::Spooled(_)) {
            return Ok(());
        }
        // Every other node came from a listing, a lookup, or a change, which each store an answer.
        let Some((_, attributes)) = &found.attributes else {
            return Err(VfsError::StaleHandle);
        };
        match attributes.kind {
            Kind::File => Ok(()),
            Kind::Folder => Err(VfsError::IsAFolder),
        }
    }

    /// One write or truncate.
    async fn edit(&self, node: NodeId, edit: Edit) -> Result<(), VfsError> {
        loop {
            let bytes = {
                let state = self.state.lock().unwrap();
                let Some(found) = state.nodes.get(&node) else {
                    return Err(VfsError::StaleHandle);
                };
                found.bytes.clone()
            };
            match bytes {
                Bytes::Spooled(id) => match self.apply(id, &edit).await {
                    // A confirmed upload deleted the spool file and left the node clean, so the next pass fetches it again.
                    Err(VfsError::StaleHandle)
                        if !upload::node_holds(&self.state.lock().unwrap(), id) => {}
                    other => return other,
                },
                Bytes::Fetching(lock) => {
                    // Held until this arm ends, so the next look runs after the fetch publishes or gives up.
                    let _guard = lock.lock().await;
                }
                Bytes::Clean => {
                    if let Fetched::Published = self.fetch(node, &edit).await? {
                        return Ok(());
                    }
                }
            }
        }
    }

    /// Copies the remote bytes `edit` needs into a new spool file, applies `edit`, and publishes the file. [`Fetched::Replaced`] means this fetch did not publish, and the caller looks at the node again.
    async fn fetch(&self, node: NodeId, edit: &Edit) -> Result<Fetched, VfsError> {
        // A write needs the whole remote file. A truncate needs only the bytes it keeps, so a cut to zero fetches nothing.
        let needed = match edit {
            Edit::Write { .. } => u64::MAX,
            Edit::SetLen(size) => *size,
        };
        let limit = self.spool()?.limit;
        // An edit that ends past the whole spool fails before any provider call.
        if edit.end() > limit {
            return Err(VfsError::NoSpace);
        }
        // The fetch claims the node before its first provider call, so a burst of first writes asks the provider once.
        let (held, item, fresh, held_revision, at, generation) = {
            let mut state = self.state.lock().unwrap();
            let generation = state.generation;
            let Some(found) = state.nodes.get_mut(&node) else {
                return Err(VfsError::StaleHandle);
            };
            if !matches!(found.bytes, Bytes::Clean) {
                return Ok(Fetched::Replaced);
            }
            let Some(item) = found.identity.remote_id().cloned() else {
                return Err(VfsError::StaleHandle);
            };
            let fresh = found
                .attributes
                .as_ref()
                .filter(|(held, _)| held.is_fresh(generation, self.options.attributes_ttl))
                .map(|(_, attributes)| attributes.clone());
            let held_revision = found
                .attributes
                .as_ref()
                .and_then(|(_, attributes)| attributes.revision.clone());
            let fetch_lock = Arc::new(tokio::sync::Mutex::new(()));
            let held = fetch_lock.clone().try_lock_owned().unwrap();
            found.bytes = Bytes::Fetching(fetch_lock);
            (held, item, fresh, held_revision, Instant::now(), generation)
        };
        let mut attempt = FetchAttempt {
            vfs: self,
            node,
            spool: None,
            _held: held,
        };
        // A copy bases the spool on the revision of the bytes it copies, so it asks for the item before it reads when the held answer is stale. A revision taken before the read can only cause a needless conflict copy, never a lost edit.
        let fresh = match fresh {
            None if needed > 0 => {
                let answer = self.call(self.provider.item(&item)).await?;
                let asked = Asked {
                    at,
                    generation,
                    answered: Instant::now(),
                };
                let mut state = self.state.lock().unwrap();
                let Some(found) = state.nodes.get_mut(&node) else {
                    return Err(VfsError::StaleHandle);
                };
                Some(remember(&mut found.attributes, asked, answer.attributes))
            }
            fresh => fresh,
        };
        // A cut to zero copies nothing, so it keeps the revision the node holds, however old.
        let base = match needed {
            0 => held_revision,
            _ => fresh
                .as_ref()
                .and_then(|attributes| attributes.revision.clone()),
        };
        // A file the spool could never hold fails before the download, not after. A size counts only while fresh: the remote file may have shrunk.
        let stored = fresh.map_or(0, |attributes| attributes.size);
        if stored.min(needed) > limit {
            return Err(VfsError::NoSpace);
        }
        let spool_id = self.create_spool_file(node, base).await?;
        attempt.spool = Some(spool_id);
        let mut offset = 0;
        while offset < needed {
            let length = u64::from(PIECE).min(needed - offset) as u32;
            let data = self.call(self.provider.read(&item, offset, length)).await?;
            let read = data.len() as u64;
            let data = Arc::new(data);
            self.apply(spool_id, &Edit::Write { offset, data }).await?;
            offset += read;
            if read < u64::from(length) {
                break;
            }
        }
        self.apply(spool_id, edit).await?;
        let mut state = self.state.lock().unwrap();
        let published = matches!(
            state.nodes.get(&node).map(|found| &found.bytes),
            Some(Bytes::Fetching(_))
        );
        attempt.spool = None;
        if !published {
            discard(&mut state, &self.room, spool_id);
            return Ok(Fetched::Replaced);
        }
        state.nodes.get_mut(&node).unwrap().bytes = Bytes::Spooled(spool_id);
        upload::schedule(
            &mut state,
            &self.uploads,
            spool_id,
            Instant::now() + self.options.settle,
        );
        Ok(Fetched::Published)
    }

    /// Reserves room, then runs the edit on the spool file. The edit lock is not held while this waits for room, and one edit waits at most the call time limit. A spool file that is gone, because its node went away, is [`VfsError::StaleHandle`].
    async fn apply(&self, id: SpoolId, edit: &Edit) -> Result<(), VfsError> {
        let deadline = Instant::now() + self.options.call_timeout;
        let Some(edits) = self
            .state
            .lock()
            .unwrap()
            .spools
            .get(&id)
            .map(|spool| Arc::clone(&spool.edits))
        else {
            return Err(VfsError::StaleHandle);
        };
        let limit = self.spool()?.limit;
        let end = edit.end();
        // No wait makes room for a file larger than the whole spool.
        if end > limit {
            return Err(VfsError::NoSpace);
        }
        let (path, before, guard) = loop {
            // Enabled before the room check, so a wake-up that lands between the check and the wait is not lost.
            let notified = self.room.notified();
            let mut notified = std::pin::pin!(notified);
            notified.as_mut().enable();
            let guard = Arc::clone(&edits).lock_owned().await;
            {
                let mut state = self.state.lock().unwrap();
                let (growth, path, before) = {
                    let Some(spool) = state.spools.get(&id) else {
                        return Err(VfsError::StaleHandle);
                    };
                    (
                        end.saturating_sub(spool.size),
                        spool.path.clone(),
                        spool.size,
                    )
                };
                if state.spool_used + growth <= limit {
                    state.spool_used += growth;
                    state.spools.get_mut(&id).unwrap().reserved = growth;
                    // A fetch's own writes land before the node holds the spool file, and do not count as a change of the file the node shows.
                    if upload::node_holds(&state, id) {
                        upload::bytes_changed(&mut state, id);
                        state.spools.get_mut(&id).unwrap().editing = true;
                    }
                    break (path, before, guard);
                }
            }
            drop(guard);
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return Err(VfsError::NoSpace);
            }
        };
        let state = Arc::clone(&self.state);
        let room = Arc::clone(&self.room);
        let uploads = Arc::clone(&self.uploads);
        let settle = self.options.settle;
        let edit = edit.clone();
        // The disk call owns the edit lock and does its own accounting, so a caller that is cancelled does not end the edit before the disk does.
        let joined = tokio::task::spawn_blocking(move || {
            let _guard = guard;
            let result = match &edit {
                Edit::Write { offset, data } => spool::write_all_at(&path, data, *offset),
                Edit::SetLen(size) => spool::set_len(&path, *size),
            };
            let length = match (&result, &edit) {
                (Ok(()), Edit::Write { .. }) => before.max(end),
                (Ok(()), Edit::SetLen(size)) => *size,
                (Err(_), _) => std::fs::metadata(&path).map_or(before, |metadata| metadata.len()),
            };
            account(&state, &room, &uploads, settle, id, length);
            result.map_err(spool::map_io)
        })
        .await;
        match joined {
            Ok(result) => result,
            Err(error) => Err(VfsError::Other(error.to_string())),
        }
    }

    /// A new empty file in the spool folder for `node`, based on the remote revision `base`, recorded in the state, and its number. A number a file from an earlier mount holds is skipped.
    async fn create_spool_file(
        &self,
        node: NodeId,
        base: Option<Revision>,
    ) -> Result<SpoolId, VfsError> {
        let spool = self.spool()?;
        loop {
            let number = self
                .next_spool
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = spool.folder.join(number.to_string());
            let opened = path.clone();
            match tokio::task::spawn_blocking(move || spool::create_named(&opened)).await {
                Ok(Ok(())) => {
                    let id = SpoolId(number);
                    let file = SpoolFile {
                        path,
                        size: 0,
                        reserved: 0,
                        modified: SystemTime::now(),
                        edits: Arc::new(tokio::sync::Mutex::new(())),
                        node,
                        base,
                        version: 0,
                        editing: false,
                        due: Instant::now(),
                        attempt: None,
                        failure: None,
                    };
                    self.state.lock().unwrap().spools.insert(id, file);
                    return Ok(id);
                }
                Ok(Err(error)) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Ok(Err(error)) => return Err(spool::map_io(error)),
                Err(error) => return Err(VfsError::Other(error.to_string())),
            }
        }
    }

    /// Renames a new file. Nothing reaches the provider. A new file renamed over an item takes that item's id, so its bytes become the item's next bytes and the item keeps its id (decision 0005, choice 9). The node the item had is retired. A handle still open on it reads the old bytes until the new ones upload: once they have, that handle reads the item's id, and the id has the new bytes.
    fn rename_new(
        &self,
        node: NodeId,
        to: &ItemId,
        new_name: &str,
        target: Option<Holder>,
        asked: Asked,
    ) {
        let mut state = self.state.lock().unwrap();
        // A new file is always spooled.
        let Bytes::Spooled(spool_id) = state.nodes[&node].bytes else {
            unreachable!("a new file has a spool file");
        };
        match target {
            Some(Holder::Item(item)) => {
                // The latest answer the core holds about the item: its node's own, or the listing's when that is newer. The new file goes on from those facts (choice 9: the revision the core holds at the rename).
                let target_node = node_for(&mut state, item.id.clone());
                let held = &mut state.nodes.get_mut(&target_node).unwrap().attributes;
                remember(held, asked, item.attributes.clone());
                let held = held.clone();
                mark_removed(&mut state, &self.room, item.id.clone());
                unlist(&mut state, &self.rules, node);
                state.nodes.get_mut(&node).unwrap().identity = Identity::Item {
                    id: item.id.clone(),
                    place: Some(Place {
                        folder: to.clone(),
                        name: unescape(&item.name, &self.rules),
                    }),
                };
                state.by_item.insert(item.id, node);
                state.spools.get_mut(&spool_id).unwrap().base = held
                    .as_ref()
                    .and_then(|(_, attributes)| attributes.revision.clone());
                state.nodes.get_mut(&node).unwrap().attributes = held;
            }
            replaced => {
                if let Some(Holder::New { node: other, .. }) = replaced {
                    unname(&mut state, &self.rules, &self.room, other);
                }
                unlist(&mut state, &self.rules, node);
                list(&mut state, &self.rules, node, to, new_name);
            }
        }
        // The name is part of what a create sends, so a try that already started stops and the file waits again.
        upload::stop_attempt(&mut state, spool_id);
        upload::schedule(
            &mut state,
            &self.uploads,
            spool_id,
            Instant::now() + self.options.settle,
        );
    }
}

impl Drop for Vfs {
    fn drop(&mut self) {
        if self.spool.is_none() {
            return;
        }
        let mut state = self.state.lock().unwrap();
        // A spool file that sends nothing is deleted. One that still has to reach the remote stays on the disk.
        let local_only: Vec<SpoolId> = state
            .nodes
            .values()
            .filter_map(|node| match node.bytes {
                Bytes::Spooled(id) if self.sends(&state, id).is_none() => Some(id),
                _ => None,
            })
            .collect();
        for id in local_only {
            discard(&mut state, &self.room, id);
        }
    }
}

/// How a fetch ended when it did not fail.
enum Fetched {
    /// The node's bytes are the new spool file, with the edit in it.
    Published,

    /// Another fetch got there first, or the node is gone. The edit is not in the node's bytes yet.
    Replaced,
}

#[derive(Clone)]
enum Edit {
    /// Shared, so an edit that has to look at its node again does not copy its bytes.
    Write {
        offset: u64,
        data: Arc<Vec<u8>>,
    },
    SetLen(u64),
}

impl Edit {
    /// How long the file is at least once the edit is done.
    fn end(&self) -> u64 {
        match self {
            Edit::Write { offset, data } => offset.saturating_add(data.len() as u64),
            Edit::SetLen(size) => *size,
        }
    }
}

enum ReadSource {
    Spool { id: SpoolId, path: PathBuf },
    Remote(ItemId),
}

/// A first edit that fails or is cancelled leaves the file as the remote has it. The next read asks the provider.
struct FetchAttempt<'a> {
    vfs: &'a Vfs,
    node: NodeId,

    /// The spool file this attempt made, until it is published or discarded.
    spool: Option<SpoolId>,

    /// The fetch lock. A field is dropped after `drop` runs, so a writer waiting on the lock sees the node clean again, never mid-fetch.
    _held: tokio::sync::OwnedMutexGuard<()>,
}

impl Drop for FetchAttempt<'_> {
    fn drop(&mut self) {
        let mut state = self.vfs.state.lock().unwrap();
        // While the attempt lives, its node is fetching or gone: nothing else moves a node out of `Fetching`.
        if let Some(found) = state.nodes.get_mut(&self.node) {
            if matches!(found.bytes, Bytes::Fetching(_)) {
                found.bytes = Bytes::Clean;
            }
        }
        if let Some(id) = self.spool.take() {
            discard(&mut state, &self.vfs.room, id);
        }
    }
}

/// Settles a disk call: the spool file is `length` bytes long now, in place of its size and the room the call reserved. Its modified time moves even when the call failed, because a write that fails partway may still have changed bytes. The edit no longer holds the file, and when the node holds it the file waits out the settle time.
fn account(
    state: &Mutex<State>,
    room: &Notify,
    uploads: &Notify,
    settle: Duration,
    id: SpoolId,
    length: u64,
) {
    let mut state = state.lock().unwrap();
    let State {
        spools, spool_used, ..
    } = &mut *state;
    let Some(spool) = spools.get_mut(&id) else {
        return;
    };
    let charged = spool.size + spool.reserved;
    *spool_used = *spool_used - charged + length;
    if length < charged {
        room.notify_waiters();
    }
    spool.size = length;
    spool.reserved = 0;
    spool.modified = SystemTime::now();
    spool.editing = false;
    if upload::node_holds(&state, id) {
        upload::schedule(&mut state, uploads, id, Instant::now() + settle);
    }
}

/// A spooled node's size and modified time, when it has a spool file.
fn spool_attributes(state: &State, node: NodeId) -> Option<Attributes> {
    let found = state.nodes.get(&node)?;
    let Bytes::Spooled(id) = &found.bytes else {
        return None;
    };
    let spool = state.spools.get(id)?;
    Some(Attributes {
        kind: Kind::File,
        size: spool.size,
        modified: spool.modified,
        revision: None,
    })
}

/// The name a new file shows under.
fn new_file_name(state: &State, rules: &NameRules, node: NodeId) -> String {
    let Identity::New(place) = &state.nodes[&node].identity else {
        unreachable!("only new files are listed as new files");
    };
    escape(&place.name, rules)
}

/// The remote items of `folder` as the mount shows them when a new file holds a name one of them shows under: the new file keeps the name, and the remote item gets a suffix. `None` when no name collides, so `listing.shown` stands.
fn renamed_items(
    state: &State,
    rules: &NameRules,
    folder: &ItemId,
    listing: &Listing,
) -> Option<Vec<Item>> {
    let held = state.new_files.get(folder)?;
    // Walk the smaller side: a copy of many files into an empty folder must not check each new name against every earlier one.
    let collides = if held.len() <= listing.index.len() {
        held.keys().any(|key| listing.index.contains_key(key))
    } else {
        listing.index.keys().any(|key| held.contains_key(key))
    };
    if !collides {
        return None;
    }
    let names: Vec<String> = held
        .values()
        .map(|node| new_file_name(state, rules, *node))
        .collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut remote = listing.remote.to_vec();
    unique_names(&mut remote, rules, &names);
    Some(remote)
}

fn resolve(
    state: &State,
    rules: &NameRules,
    folder: &ItemId,
    listing: &Listing,
    name: &str,
) -> Option<Holder> {
    let wanted = (rules.key)(name);
    if let Some(node) = state
        .new_files
        .get(folder)
        .and_then(|held| held.get(&wanted))
    {
        return Some(Holder::New {
            node: *node,
            name: new_file_name(state, rules, *node),
        });
    }
    let item = match renamed_items(state, rules, folder, listing) {
        None => listing
            .index
            .get(&wanted)
            .map(|at| listing.shown[*at].clone()),
        Some(items) => items
            .into_iter()
            .find(|item| (rules.key)(&item.name) == wanted),
    };
    item.map(Holder::Item)
}

/// The entry a holder shows as. An item gets its node, and the node keeps what the listing said about it and where it was shown. Counts nothing.
fn entry_for(
    state: &mut State,
    rules: &NameRules,
    folder: &ItemId,
    listing: &Listing,
    holder: Holder,
) -> Entry {
    match holder {
        Holder::New { node, name } => Entry {
            node,
            name,
            attributes: spool_attributes(state, node).unwrap(),
        },
        Holder::Item(item) => {
            let node = node_for(state, item.id);
            let found = state.nodes.get_mut(&node).unwrap();
            set_place(found, folder, unescape(&item.name, rules));
            let held = remember(&mut found.attributes, listing.asked, item.attributes);
            Entry {
                node,
                name: item.name,
                attributes: spool_attributes(state, node).unwrap_or(held),
            }
        }
    }
}

fn set_place(node: &mut Node, folder: &ItemId, remote_name: String) {
    if let Identity::Item { place, .. } = &mut node.identity {
        *place = Some(Place {
            folder: folder.clone(),
            name: remote_name,
        });
    }
}

/// This mount removed `id`, or replaced it by a rename. Its node keeps the id for the handles still open on it, and its bytes go nowhere.
fn mark_removed(state: &mut State, room: &Notify, id: ItemId) {
    let Some(node) = state.by_item.remove(&id) else {
        return;
    };
    if let Bytes::Spooled(spool_id) = state.nodes[&node].bytes {
        upload::stop_attempt(state, spool_id);
    }
    state.nodes.get_mut(&node).unwrap().identity = Identity::Removed(id);
    drop_unreferenced(state, room, node);
}

/// Gives a new file the name `name`, as the OS sent it, in `folder`.
fn list(state: &mut State, rules: &NameRules, node: NodeId, folder: &ItemId, name: &str) {
    let remote_name = unescape(name, rules);
    // The same key `unlist` takes it out by.
    let key = (rules.key)(&escape(&remote_name, rules));
    state.nodes.get_mut(&node).unwrap().identity = Identity::New(Place {
        folder: folder.clone(),
        name: remote_name,
    });
    state
        .new_files
        .entry(folder.clone())
        .or_default()
        .insert(key, node);
}

/// Takes a new file out of its folder's new files.
fn unlist(state: &mut State, rules: &NameRules, node: NodeId) {
    let Identity::New(place) = &state.nodes[&node].identity else {
        return;
    };
    let folder = place.folder.clone();
    let key = (rules.key)(&escape(&place.name, rules));
    let held = state.new_files.get_mut(&folder).unwrap();
    held.remove(&key);
    if held.is_empty() {
        state.new_files.remove(&folder);
    }
}

/// A new file loses its name, as a remove or a rename over it does. Its open handles still work, and its node goes once nothing holds it.
fn unname(state: &mut State, rules: &NameRules, room: &Notify, node: NodeId) {
    if let Bytes::Spooled(spool_id) = state.nodes[&node].bytes {
        upload::stop_attempt(state, spool_id);
    }
    unlist(state, rules, node);
    state.nodes.get_mut(&node).unwrap().identity = Identity::Unnamed;
    drop_unreferenced(state, room, node);
}

fn discard(state: &mut State, room: &Notify, id: SpoolId) {
    let Some(spool) = state.spools.remove(&id) else {
        return;
    };
    state.spool_used -= spool.size + spool.reserved;
    state.queue.remove(&(spool.due, id));
    room.notify_waiters();
    // Unlinking is one quick call, and the sync callers (`release`, `forget`) cannot wait. When any caller returns, the bytes are gone.
    let _ = std::fs::remove_file(&spool.path);
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
    edit(Arc::make_mut(&mut listing.remote));
    *listing = Listing::new(listing.asked, Arc::clone(&listing.remote), rules);
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
            identity: Identity::Item {
                id: item,
                place: None,
            },
            lookups: 0,
            handles: 0,
            writers: 0,
            attributes: None,
            bytes: Bytes::Clean,
        },
    );
    id
}

/// Stores an answer from the provider and returns the latest answer held. An answer asked for earlier never replaces one asked for later: a lookup can carry a listing that is older than the node's own answer, and a slow call can end after a later one.
fn remember<T: Clone>(held: &mut Option<(Asked, T)>, asked: Asked, answer: T) -> T {
    if held
        .as_ref()
        .is_some_and(|(kept, _)| (kept.generation, kept.at) > (asked.generation, asked.at))
    {
        return held.as_ref().unwrap().1.clone();
    }
    *held = Some((asked, answer.clone()));
    answer
}

fn drop_unreferenced(state: &mut State, room: &Notify, node: NodeId) {
    if node == Vfs::ROOT {
        return;
    }
    let Some(found) = state.nodes.get(&node) else {
        return;
    };
    if found.lookups != 0 || found.handles != 0 {
        return;
    }
    // A spooled item or new file is where its bytes go, so it stays after the OS forgets it.
    if matches!(found.bytes, Bytes::Spooled(_))
        && matches!(found.identity, Identity::Item { .. } | Identity::New(_))
    {
        return;
    }
    let found = state.nodes.remove(&node).unwrap();
    if let Some(id) = found.identity.remote_id() {
        if state.by_item.get(id) == Some(&node) {
            state.by_item.remove(id);
        }
    }
    if let Bytes::Spooled(id) = found.bytes {
        discard(state, room, id);
    }
}

fn insert_handle(state: &mut State, opened: Opened) -> HandleId {
    let handle = HandleId(state.next_handle);
    state.next_handle += 1;
    state.handles.insert(handle, opened);
    handle
}
