use std::sync::Mutex;
use std::time::{Duration, UNIX_EPOCH};

use crate::{Attributes, Item, ItemId, Kind, Provider, Space, VfsError};

struct Stored {
    item: Item,
    parent: Option<ItemId>,
    contents: Vec<u8>,
}

struct Inner {
    stored: Vec<Stored>,
    added: u64,
    delay: Duration,
    error: Option<VfsError>,
    in_flight: usize,
    peak: usize,
}

/// A provider that keeps its tree in memory, for tests. Like Google Drive, it allows two items with one name in a folder and any character in a name.
///
/// A call works out its answer when it starts and returns it after the delay. So a change made while a call waits shows up on the next call, as it would with a real remote.
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
                },
            },
            parent: None,
            contents: Vec::new(),
        };
        Self {
            inner: Mutex::new(Inner {
                stored: vec![root],
                added: 0,
                delay: Duration::ZERO,
                error: None,
                in_flight: 0,
                peak: 0,
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

    /// Moves an item and gives it a new name. Its ID stays the same.
    pub fn rename(&self, id: &ItemId, new_parent: &ItemId, new_name: &str) {
        let mut inner = self.inner.lock().unwrap();
        let stored = inner.stored.iter_mut().find(|stored| stored.item.id == *id);
        let stored = stored.expect("no item has this ID");
        stored.item.name = new_name.to_string();
        stored.parent = Some(new_parent.clone());
    }

    /// Removes one item.
    pub fn remove(&self, id: &ItemId) {
        let mut inner = self.inner.lock().unwrap();
        inner.stored.retain(|stored| stored.item.id != *id);
    }

    /// Makes every later provider call wait this long before it answers.
    pub fn set_delay(&self, delay: Duration) {
        self.inner.lock().unwrap().delay = delay;
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

    fn add(&self, parent: &ItemId, name: &str, kind: Kind, contents: &[u8]) -> ItemId {
        let mut inner = self.inner.lock().unwrap();
        inner.added += 1;
        let number = inner.added;
        let id = ItemId(format!("item-{number}"));
        inner.stored.push(Stored {
            item: Item {
                id: id.clone(),
                name: name.to_string(),
                attributes: Attributes {
                    kind,
                    size: contents.len() as u64,
                    modified: UNIX_EPOCH + Duration::from_secs(number),
                },
            },
            parent: Some(parent.clone()),
            contents: contents.to_vec(),
        });
        id
    }

    /// Runs one provider call: works out the answer now, then returns it after the delay.
    async fn answer<T>(
        &self,
        read: impl FnOnce(&[Stored]) -> Result<T, VfsError>,
    ) -> Result<T, VfsError> {
        let (delay, result) = {
            let mut inner = self.inner.lock().unwrap();
            inner.in_flight += 1;
            inner.peak = inner.peak.max(inner.in_flight);
            let result = match &inner.error {
                Some(error) => Err(error.clone()),
                None => read(&inner.stored),
            };
            (inner.delay, result)
        };
        let _in_flight = InFlight(&self.inner);
        tokio::time::sleep(delay).await;
        result
    }
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
        self.answer(|stored| Ok(find(stored, id)?.item.clone()))
            .await
    }

    async fn list(&self, folder: &ItemId) -> Result<Vec<Item>, VfsError> {
        self.answer(|stored| {
            if find(stored, folder)?.item.attributes.kind != Kind::Folder {
                return Err(VfsError::NotAFolder);
            }
            Ok(stored
                .iter()
                .filter(|stored| stored.parent.as_ref() == Some(folder))
                .map(|stored| stored.item.clone())
                .collect())
        })
        .await
    }

    async fn read(&self, id: &ItemId, offset: u64, length: u32) -> Result<Vec<u8>, VfsError> {
        self.answer(|stored| {
            let file = find(stored, id)?;
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
}
