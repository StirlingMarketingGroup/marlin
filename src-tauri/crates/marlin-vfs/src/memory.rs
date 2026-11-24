use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::{Attributes, Item, ItemId, Kind, Progress, Provider, Revision, Space, VfsError};

struct Stored {
    item: Item,
    parent: Option<ItemId>,
    contents: Vec<u8>,
}

struct Inner {
    stored: Vec<Stored>,
    added: u64,
    /// Goes up on every byte change, and becomes `rev-{n}`.
    revisions: u64,
    delay: Duration,
    upload_delay: Duration,
    error: Option<VfsError>,
    in_flight: usize,
    peak: usize,
    calls: usize,
    uploads: usize,
    rename_changes_ids: bool,
    hide_revisions: bool,
    /// A byte call changes the tree, then never answers.
    lose_answers: bool,
    largest_read: u32,
}

/// A provider that keeps its tree in memory, for tests. Like Google Drive, it allows two items with one name in a folder and any character in a name.
///
/// A metadata or read call works out its answer when it starts and returns it after the delay. So a change made while a call waits shows up on the next call, as it would with a real remote. A byte call checks for a failure and changes the tree only when its upload delay ends.
pub struct MemoryProvider {
    inner: Mutex<Inner>,
}

/// Counts one provider call as in flight until it is dropped, which also covers a call the caller cancels.
struct InFlight<'a>(&'a Mutex<Inner>);

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.lock().unwrap().in_flight -= 1;
    }
}

impl MemoryProvider {
    /// An empty root folder whose ID is `root`.
    pub fn new() -> Self {
        let root = Stored {
            item: Item {
                id: ItemId("root".to_string()),
                name: String::new(),
                attributes: Attributes {
                    kind: Kind::Folder,
                    size: 0,
                    modified: UNIX_EPOCH,
                    revision: None,
                    read_only: false,
                },
            },
            parent: None,
            contents: Vec::new(),
        };
        Self {
            inner: Mutex::new(Inner {
                stored: vec![root],
                added: 0,
                revisions: 0,
                delay: Duration::ZERO,
                upload_delay: Duration::ZERO,
                error: None,
                in_flight: 0,
                peak: 0,
                calls: 0,
                uploads: 0,
                rename_changes_ids: false,
                hide_revisions: false,
                lose_answers: false,
                largest_read: 0,
            }),
        }
    }

    /// Adds a folder under `parent` and returns its new ID.
    pub fn add_folder(&self, parent: &ItemId, name: &str) -> ItemId {
        self.add(parent, name, Kind::Folder, &[])
    }

    /// Adds a file under `parent` and returns its new ID.
    pub fn add_file(&self, parent: &ItemId, name: &str, contents: &[u8]) -> ItemId {
        self.add(parent, name, Kind::File, contents)
    }

    /// Replaces a file's bytes. Its size follows, its modified time moves forward, and it gets a new revision.
    pub fn write(&self, id: &ItemId, contents: &[u8]) {
        let mut inner = self.inner.lock().unwrap();
        let revision = next_revision(&mut inner);
        let stored = inner.stored.iter_mut().find(|stored| stored.item.id == *id);
        let stored = stored.expect("no item has this ID");
        stored.contents = contents.to_vec();
        stored.item.attributes.size = contents.len() as u64;
        stored.item.attributes.modified += Duration::from_secs(1);
        stored.item.attributes.revision = Some(revision);
    }

    /// Moves an item and gives it a new name behind the mount's back. No provider call is counted, and its id stays the same.
    pub fn remote_rename(&self, id: &ItemId, new_parent: &ItemId, new_name: &str) {
        let mut inner = self.inner.lock().unwrap();
        let stored = inner.stored.iter_mut().find(|stored| stored.item.id == *id);
        let stored = stored.expect("no item has this ID");
        stored.item.name = new_name.to_string();
        stored.parent = Some(new_parent.clone());
    }

    /// While on, `replace_bytes` of the file answers [`VfsError::AccessDenied`], as Google Drive does for a file the account may only view. `item` and `list` report it. The other calls do not look at it.
    pub fn set_read_only(&self, id: &ItemId, on: bool) {
        let mut inner = self.inner.lock().unwrap();
        let stored = inner.stored.iter_mut().find(|stored| stored.item.id == *id);
        let stored = stored.expect("no item has this ID");
        stored.item.attributes.read_only = on;
    }

    /// Removes one item behind the mount's back. No provider call is counted.
    pub fn remote_remove(&self, id: &ItemId) {
        let mut inner = self.inner.lock().unwrap();
        inner.stored.retain(|stored| stored.item.id != *id);
    }

    /// While on, [`Provider::rename`] gives the moved item a new id, as a remote whose ids are paths would: the replaced item's id when it replaces one, and a fresh id otherwise. Off by default.
    pub fn rename_changes_ids(&self, on: bool) {
        self.inner.lock().unwrap().rename_changes_ids = on;
    }

    /// While on, `item` and `list` give no revision, as a provider without revisions would. Off by default.
    pub fn hide_revisions(&self, on: bool) {
        self.inner.lock().unwrap().hide_revisions = on;
    }

    /// While on, a byte call changes the tree as usual and then never answers, as an upload whose answer a crash lost.
    pub fn lose_upload_answers(&self, on: bool) {
        self.inner.lock().unwrap().lose_answers = on;
    }

    /// Makes every later provider call wait this long before it answers. The two byte calls wait for [`Self::set_upload_delay`] instead.
    pub fn set_delay(&self, delay: Duration) {
        self.inner.lock().unwrap().delay = delay;
    }

    /// Makes [`Provider::create_file`] and [`Provider::replace_bytes`] wait this long before they change the tree. [`Self::set_delay`] does not apply to them.
    pub fn set_upload_delay(&self, delay: Duration) {
        self.inner.lock().unwrap().upload_delay = delay;
    }

    /// How many byte calls have started.
    pub fn uploads(&self) -> usize {
        self.inner.lock().unwrap().uploads
    }

    /// While set, every provider call fails with this error.
    pub fn fail_with(&self, error: Option<VfsError>) {
        self.inner.lock().unwrap().error = error;
    }

    /// How many provider calls are running right now.
    pub fn in_flight(&self) -> usize {
        self.inner.lock().unwrap().in_flight
    }

    /// The most provider calls that have been in flight at once.
    pub fn peak_calls(&self) -> usize {
        self.inner.lock().unwrap().peak
    }

    /// How many provider calls have started.
    pub fn calls(&self) -> usize {
        self.inner.lock().unwrap().calls
    }

    /// The largest `length` any [`Provider::read`] call asked for.
    pub fn largest_read(&self) -> u32 {
        self.inner.lock().unwrap().largest_read
    }

    /// The stored bytes of an item, so a test can check the remote was not changed.
    pub fn contents(&self, id: &ItemId) -> Vec<u8> {
        let inner = self.inner.lock().unwrap();
        find(&inner.stored, id)
            .expect("no item has this ID")
            .contents
            .clone()
    }

    fn add(&self, parent: &ItemId, name: &str, kind: Kind, contents: &[u8]) -> ItemId {
        let mut inner = self.inner.lock().unwrap();
        allocate(&mut inner, Some(parent.clone()), name, kind, contents).id
    }

    /// Runs one provider call: works out the answer now, then returns it after the delay.
    async fn answer<T>(
        &self,
        op: impl FnOnce(&mut Inner) -> Result<T, VfsError>,
    ) -> Result<T, VfsError> {
        let (delay, result) = {
            let mut inner = self.inner.lock().unwrap();
            inner.in_flight += 1;
            inner.calls += 1;
            inner.peak = inner.peak.max(inner.in_flight);
            let error = inner.error.clone();
            let result = match error {
                Some(error) => Err(error),
                None => op(&mut inner),
            };
            (inner.delay, result)
        };
        let _in_flight = InFlight(&self.inner);
        tokio::time::sleep(delay).await;
        result
    }

    /// Counts the call now, waits the upload delay, then checks `fail_with` and changes the tree. An upload changes the remote only when its last byte lands, so a call dropped while it waits changes nothing. Halfway through the delay it reports half of `from` as sent, as a real upload reports progress on its way.
    async fn upload<T>(
        &self,
        from: &Path,
        progress: &Progress,
        op: impl FnOnce(&mut Inner) -> Result<T, VfsError>,
    ) -> Result<T, VfsError> {
        let delay = {
            let mut inner = self.inner.lock().unwrap();
            inner.in_flight += 1;
            inner.calls += 1;
            inner.uploads += 1;
            inner.peak = inner.peak.max(inner.in_flight);
            inner.upload_delay
        };
        let _in_flight = InFlight(&self.inner);
        tokio::time::sleep(delay / 2).await;
        progress(std::fs::metadata(from).map_or(0, |metadata| metadata.len()) / 2);
        tokio::time::sleep(delay - delay / 2).await;
        let (lose, result) = {
            let mut inner = self.inner.lock().unwrap();
            let result = match inner.error.clone() {
                Some(error) => Err(error),
                None => op(&mut inner),
            };
            (inner.lose_answers, result)
        };
        if lose && result.is_ok() {
            return std::future::pending().await;
        }
        result
    }
}

/// Stores a new item and returns it. Its id and its modified time come from `added`.
fn allocate(
    inner: &mut Inner,
    parent: Option<ItemId>,
    name: &str,
    kind: Kind,
    contents: &[u8],
) -> Item {
    inner.added += 1;
    let number = inner.added;
    let item = Item {
        id: ItemId(format!("item-{number}")),
        name: name.to_string(),
        attributes: Attributes {
            kind,
            size: contents.len() as u64,
            modified: UNIX_EPOCH + Duration::from_secs(number),
            revision: (kind == Kind::File).then(|| next_revision(inner)),
            read_only: false,
        },
    };
    inner.stored.push(Stored {
        item: item.clone(),
        parent,
        contents: contents.to_vec(),
    });
    item
}

impl Default for MemoryProvider {
    fn default() -> Self {
        Self::new()
    }
}

fn find<'a>(stored: &'a [Stored], id: &ItemId) -> Result<&'a Stored, VfsError> {
    stored
        .iter()
        .find(|stored| stored.item.id == *id)
        .ok_or(VfsError::NotFound)
}

#[async_trait::async_trait]
impl Provider for MemoryProvider {
    fn root(&self) -> ItemId {
        ItemId("root".to_string())
    }

    async fn item(&self, id: &ItemId) -> Result<Item, VfsError> {
        self.answer(|inner| Ok(shown(inner, &find(&inner.stored, id)?.item)))
            .await
    }

    async fn list(&self, folder: &ItemId) -> Result<Vec<Item>, VfsError> {
        self.answer(|inner| {
            if find(&inner.stored, folder)?.item.attributes.kind != Kind::Folder {
                return Err(VfsError::NotAFolder);
            }
            Ok(inner
                .stored
                .iter()
                .filter(|stored| stored.parent.as_ref() == Some(folder))
                .map(|stored| shown(inner, &stored.item))
                .collect())
        })
        .await
    }

    async fn read(&self, id: &ItemId, offset: u64, length: u32) -> Result<Vec<u8>, VfsError> {
        self.answer(|inner| {
            inner.largest_read = inner.largest_read.max(length);
            let file = find(&inner.stored, id)?;
            if file.item.attributes.kind != Kind::File {
                return Err(VfsError::IsAFolder);
            }
            let size = file.contents.len() as u64;
            let start = offset.min(size) as usize;
            let end = (offset.saturating_add(u64::from(length))).min(size) as usize;
            Ok(file.contents[start..end].to_vec())
        })
        .await
    }

    async fn space(&self) -> Result<Space, VfsError> {
        self.answer(|_| {
            Ok(Space {
                total: 1_000_000,
                free: 250_000,
            })
        })
        .await
    }

    async fn locate(&self, reference: &str) -> Result<Option<Vec<ItemId>>, VfsError> {
        self.answer(|inner| {
            let mut at = find(&inner.stored, &ItemId(reference.to_string()))?;
            let mut chain = Vec::new();
            while let Some(parent) = &at.parent {
                chain.push(at.item.id.clone());
                // A folder removed behind the mount's back leaves what it held in no folder.
                let Ok(found) = find(&inner.stored, parent) else {
                    return Ok(None);
                };
                at = found;
            }
            chain.reverse();
            Ok(Some(chain))
        })
        .await
    }

    fn reference(&self, id: &ItemId) -> Option<String> {
        (*id != self.root()).then(|| id.0.clone())
    }

    fn can_write(&self) -> bool {
        true
    }

    async fn create_folder(&self, parent: &ItemId, name: &str) -> Result<Item, VfsError> {
        self.answer(|inner| {
            if find(&inner.stored, parent)?.item.attributes.kind != Kind::Folder {
                return Err(VfsError::NotAFolder);
            }
            Ok(allocate(
                inner,
                Some(parent.clone()),
                name,
                Kind::Folder,
                &[],
            ))
        })
        .await
    }

    async fn remove(&self, id: &ItemId, parent: &ItemId) -> Result<(), VfsError> {
        self.answer(|inner| {
            if find(&inner.stored, id)?.parent.as_ref() != Some(parent) {
                return Err(VfsError::NotFound);
            }
            let holds_any = |stored: &Stored| stored.parent.as_ref() == Some(id);
            if inner.stored.iter().any(holds_any) {
                return Err(VfsError::NotEmpty);
            }
            inner.stored.retain(|stored| stored.item.id != *id);
            Ok(())
        })
        .await
    }

    async fn rename(
        &self,
        id: &ItemId,
        from: &ItemId,
        to: &ItemId,
        name: &str,
        replace: Option<&ItemId>,
    ) -> Result<Item, VfsError> {
        self.answer(|inner| {
            if find(&inner.stored, id)?.parent.as_ref() != Some(from) {
                return Err(VfsError::NotFound);
            }
            if find(&inner.stored, to)?.item.attributes.kind != Kind::Folder {
                return Err(VfsError::NotAFolder);
            }
            // Walk up from `to`: it must not be `id` or sit inside it.
            let mut above = Some(to.clone());
            while let Some(folder) = above {
                if folder == *id {
                    return Err(VfsError::NotSupported);
                }
                above = find(&inner.stored, &folder)
                    .ok()
                    .and_then(|stored| stored.parent.clone());
            }
            if let Some(replace) = replace {
                if find(&inner.stored, replace)?.parent.as_ref() != Some(to) {
                    return Err(VfsError::NotFound);
                }
                let holds_any = |stored: &Stored| stored.parent.as_ref() == Some(replace);
                if inner.stored.iter().any(holds_any) {
                    return Err(VfsError::NotEmpty);
                }
                inner.stored.retain(|stored| stored.item.id != *replace);
            }
            let fresh_id = inner.rename_changes_ids.then(|| {
                replace.cloned().unwrap_or_else(|| {
                    inner.added += 1;
                    ItemId(format!("item-{}", inner.added))
                })
            });
            let stored = inner.stored.iter_mut().find(|stored| stored.item.id == *id);
            let stored = stored.expect("the item was found before anything changed");
            stored.parent = Some(to.clone());
            stored.item.name = name.to_string();
            if let Some(fresh_id) = &fresh_id {
                stored.item.id = fresh_id.clone();
            }
            let moved = stored.item.clone();
            if let Some(fresh_id) = fresh_id {
                // What a folder holds follows it to its new id.
                for child in &mut inner.stored {
                    if child.parent.as_ref() == Some(id) {
                        child.parent = Some(fresh_id.clone());
                    }
                }
            }
            Ok(moved)
        })
        .await
    }

    async fn create_file(
        &self,
        parent: &ItemId,
        name: &str,
        from: &Path,
        modified: SystemTime,
        progress: &Progress,
    ) -> Result<Item, VfsError> {
        self.upload(from, progress, |inner| {
            if find(&inner.stored, parent)?.item.attributes.kind != Kind::Folder {
                return Err(VfsError::NotAFolder);
            }
            let bytes = std::fs::read(from).map_err(|error| VfsError::Other(error.to_string()))?;
            allocate(inner, Some(parent.clone()), name, Kind::File, &bytes);
            let stored = inner.stored.last_mut().expect("the file was just added");
            stored.item.attributes.modified = modified;
            progress(bytes.len() as u64);
            Ok(stored.item.clone())
        })
        .await
    }

    async fn replace_bytes(
        &self,
        id: &ItemId,
        from: &Path,
        base: &Revision,
        modified: SystemTime,
        progress: &Progress,
    ) -> Result<Item, VfsError> {
        self.upload(from, progress, |inner| {
            let bytes = std::fs::read(from).map_err(|error| VfsError::Other(error.to_string()))?;
            let stored = find(&inner.stored, id)?;
            if stored.item.attributes.kind != Kind::File {
                return Err(VfsError::IsAFolder);
            }
            if stored.item.attributes.read_only {
                return Err(VfsError::AccessDenied);
            }
            if stored.item.attributes.revision.as_ref() != Some(base) {
                return Err(VfsError::Conflict);
            }
            let revision = next_revision(inner);
            let stored = inner
                .stored
                .iter_mut()
                .find(|stored| stored.item.id == *id)
                .expect("the file was found before the revision changed");
            stored.contents = bytes.clone();
            stored.item.attributes.size = bytes.len() as u64;
            stored.item.attributes.modified = modified;
            stored.item.attributes.revision = Some(revision);
            progress(bytes.len() as u64);
            Ok(stored.item.clone())
        })
        .await
    }
}

/// An item as `item` and `list` give it.
fn shown(inner: &Inner, item: &Item) -> Item {
    let mut item = item.clone();
    if inner.hide_revisions {
        item.attributes.revision = None;
    }
    item
}

/// The next revision a byte change gives a file.
fn next_revision(inner: &mut Inner) -> Revision {
    inner.revisions += 1;
    Revision(format!("rev-{}", inner.revisions))
}
