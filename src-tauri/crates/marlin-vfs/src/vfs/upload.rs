use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tokio::sync::Notify;
use tokio::time::Instant;

use crate::{spool, Item, ItemId, Revision, VfsError};

use super::record::save_record;
use super::{
    change_listing, discard, drop_unreferenced, note_changed, remember, unlist, unname, Asked,
    Bytes, Identity, NodeId, Place, Sent, SpoolId, State, Vfs, PIECE,
};

/// How long the first retry waits. Each later wait is twice the one before, up to [`RETRY_MAX`].
const RETRY_START: Duration = Duration::from_secs(5);

/// The longest a retry waits.
const RETRY_MAX: Duration = Duration::from_secs(5 * 60);

/// One file the spool holds that should still reach the remote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsent {
    pub node: NodeId,

    /// The folder of the file whose edit is unsent.
    pub folder: ItemId,

    /// The name of the file whose edit is unsent, as the remote gets it: the shown name, unescaped. While a conflict copy uploads, the copy goes up under another name.
    pub name: String,

    pub size: u64,

    pub state: UnsentState,
}

/// Where an [`Unsent`] file is in its upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnsentState {
    /// A writer has it open.
    Open,

    /// No writer has it open and no upload of it runs: it waits for its settle time or for a free upload slot.
    Waiting,

    /// An upload of its current bytes runs. `sent` is how far the provider has reported.
    Uploading { sent: u64 },

    /// The last try failed. The next one comes by itself, later.
    Failed(VfsError),
}

/// The one try a spool file has running. It stays until the try's task ends, so a second try never starts while the first one's answer may still be applied.
pub(super) struct Attempt {
    /// Something asked the try to stop: an edit, a rename of a new file, a remove, or a discard. The file is no longer shown as uploading.
    stopped: bool,

    /// How many bytes the current provider call has reported.
    sent: u64,

    /// When `sent` last grew, or when the current provider call began.
    moved: Instant,

    stop: Arc<Notify>,
}

/// What a spool file sends.
#[derive(Clone, PartialEq, Eq)]
pub(super) enum Target {
    /// New bytes for this item.
    Replace { id: ItemId },

    /// A file the remote does not have yet.
    Create { folder: ItemId, name: String },
}

/// What a provider's answer made on the remote.
enum Landed {
    /// New bytes for the item the try sent to.
    Replaced,

    /// A new item in `folder`. `gone` is the item it stands in for, when the remote had lost that one.
    Created {
        folder: ItemId,
        gone: Option<ItemId>,
    },

    /// A conflict copy in `folder`: a new item beside the one the try sent to, which keeps the remote's version. `remote` is that item as the provider answered right after, when it answered.
    Copied {
        folder: ItemId,
        remote: Option<(Asked, Item)>,
    },
}

/// One try's view of its spool file, taken when it starts.
struct Job {
    spool_id: SpoolId,
    node: NodeId,
    path: PathBuf,
    modified: SystemTime,
    version: u64,
    /// The spool file's length when the try started.
    size: u64,
    stop: Arc<Notify>,
    target: Target,
    /// An earlier run's try may have landed these bytes, so this try looks at the remote first.
    check_remote_first: bool,
}

/// Ends a try whichever way it ends, an aborted task included, and puts the file back in the queue so the uploader looks at it again.
struct AttemptGuard {
    vfs: Arc<Vfs>,
    spool_id: SpoolId,
}

impl Drop for AttemptGuard {
    fn drop(&mut self) {
        let mut state = self.vfs.state.lock().unwrap();
        let Some(spool) = state.spools.get_mut(&self.spool_id) else {
            return;
        };
        if spool.attempt.take().is_some() {
            let due = spool.due;
            schedule(&mut state, &self.vfs.uploads, self.spool_id, due);
        }
    }
}

/// Sets when `spool_id` may next be sent, keeps that one time in the queue, and wakes the uploader.
pub(super) fn schedule(state: &mut State, uploads: &Notify, spool_id: SpoolId, at: Instant) {
    let Some(spool) = state.spools.get_mut(&spool_id) else {
        return;
    };
    let before = std::mem::replace(&mut spool.due, at);
    state.queue.remove(&(before, spool_id));
    state.queue.insert((at, spool_id));
    uploads.notify_one();
}

/// Asks the running try to stop. It stays the file's attempt until its task ends.
pub(super) fn stop_attempt(state: &mut State, spool_id: SpoolId) {
    if let Some(attempt) = state
        .spools
        .get_mut(&spool_id)
        .and_then(|spool| spool.attempt.as_mut())
    {
        attempt.stopped = true;
        attempt.stop.notify_one();
    }
}

/// The spool file's bytes or modified time changed, so a try that runs is not sending the file as it is now.
pub(super) fn bytes_changed(state: &mut State, spool_id: SpoolId) {
    let Some(spool) = state.spools.get_mut(&spool_id) else {
        return;
    };
    spool.version += 1;
    spool.failure = None;
    stop_attempt(state, spool_id);
}

/// What a spool file is for, before the names that stay on this computer are left out: the item and the place the mount last showed it, or a new file's place.
pub(super) fn sends_to(state: &State, spool_id: SpoolId) -> Option<(Target, Place)> {
    if !node_holds(state, spool_id) {
        return None;
    }
    match &state.nodes[&state.spools[&spool_id].node].identity {
        Identity::Item {
            id,
            place: Some(place),
        } => Some((Target::Replace { id: id.clone() }, place.clone())),
        Identity::New(place) => Some((
            Target::Create {
                folder: place.folder.clone(),
                name: place.name.clone(),
            },
            place.clone(),
        )),
        _ => None,
    }
}

/// Whether the spool file is its node's bytes. A fetch's spool file is not, until the fetch publishes it.
pub(super) fn node_holds(state: &State, spool_id: SpoolId) -> bool {
    let Some(spool) = state.spools.get(&spool_id) else {
        return false;
    };
    state
        .nodes
        .get(&spool.node)
        .is_some_and(|node| matches!(node.bytes, Bytes::Spooled(id) if id == spool_id))
}

/// The node's bytes are the remote's again: its spool file goes, and so does the node when nothing else holds it.
fn back_to_remote(state: &mut State, room: &Notify, node: NodeId, spool_id: SpoolId) {
    discard(state, room, spool_id);
    state.nodes.get_mut(&node).unwrap().bytes = Bytes::Clean;
    drop_unreferenced(state, room, node);
}

impl Vfs {
    /// Sends spooled files until the task is aborted.
    ///
    /// The mount's owner spawns it once, after it builds the [`Vfs`] with a spool, and aborts it to stop uploading. A mount with no spool returns at once. Nothing uploads while it is not running. Aborting it stops every upload in flight, and their spool files stay.
    pub async fn run_uploads(self: Arc<Self>) {
        if self.spool.is_none() {
            return;
        }
        // Dropping the set when this task is aborted aborts every try in it.
        let mut tasks = tokio::task::JoinSet::new();
        loop {
            let next = {
                let mut state = self.state.lock().unwrap();
                let now = Instant::now();
                while tasks.len() < self.options.max_uploads {
                    let Some(&(at, spool_id)) = state.queue.first() else {
                        break;
                    };
                    if at > now {
                        break;
                    }
                    state.queue.remove(&(at, spool_id));
                    // An entry that cannot start is dropped. Whatever holds the file back queues it again when it lets go.
                    let Some((target, _)) = self.sends(&state, spool_id) else {
                        continue;
                    };
                    let spool = &state.spools[&spool_id];
                    if spool.attempt.is_some()
                        || spool.editing
                        || state.nodes[&spool.node].writers > 0
                    {
                        continue;
                    }
                    let stop = Arc::new(Notify::new());
                    let job = Job {
                        spool_id,
                        node: spool.node,
                        path: spool.path.clone(),
                        modified: spool.modified,
                        version: spool.version,
                        size: spool.size,
                        stop: Arc::clone(&stop),
                        target,
                        check_remote_first: spool.sent == Sent::EarlierRun,
                    };
                    let base = spool.base.clone();
                    let spool = state.spools.get_mut(&spool_id).unwrap();
                    spool.attempt = Some(Attempt {
                        stopped: false,
                        sent: 0,
                        moved: now,
                        stop,
                    });
                    if spool.sent == Sent::Never {
                        spool.sent = Sent::ThisRun;
                        let _ = save_record(&mut state, spool_id);
                    }
                    let guard = AttemptGuard {
                        vfs: Arc::clone(&self),
                        spool_id,
                    };
                    tasks.spawn(async move {
                        guard.vfs.clone().upload(job, base).await;
                        drop(guard);
                    });
                }
                // A due entry with no free slot must not spin this loop.
                let has_room = tasks.len() < self.options.max_uploads;
                state.queue.first().map(|(at, _)| *at).filter(|_| has_room)
            };
            tokio::select! {
                _ = self.uploads.notified() => {}
                Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
                _ = tokio::time::sleep_until(next.unwrap_or_else(Instant::now)), if next.is_some() => {}
            }
        }
    }

    /// Every spool file that should still reach the remote, in node order. Removed, unnamed, and stays-local files are left out.
    pub fn unsent(&self) -> Vec<Unsent> {
        let state = self.state.lock().unwrap();
        let mut found: Vec<Unsent> = state
            .spools
            .iter()
            .filter_map(|(&spool_id, file)| {
                let (_, place) = self.sends(&state, spool_id)?;
                let node = file.node;
                let upload = match &file.attempt {
                    Some(attempt) if !attempt.stopped => {
                        UnsentState::Uploading { sent: attempt.sent }
                    }
                    _ if state.nodes[&node].writers > 0 => UnsentState::Open,
                    _ => match &file.failure {
                        Some((error, _)) => UnsentState::Failed(error.clone()),
                        None => UnsentState::Waiting,
                    },
                };
                Some(Unsent {
                    node,
                    folder: place.folder,
                    name: place.name,
                    size: file.size,
                    state: upload,
                })
            })
            .collect();
        found.sort_by_key(|unsent| unsent.node);
        found
    }

    /// Sends a failed file now, without waiting for its retry. Any other unsent file is left as it is. A node with no unsent bytes is [`VfsError::NotFound`].
    pub fn retry_now(&self, node: NodeId) -> Result<(), VfsError> {
        let mut state = self.state.lock().unwrap();
        let Some((spool_id, _)) = self.unsent_spool(&state, node) else {
            return Err(VfsError::NotFound);
        };
        if state.spools[&spool_id].failure.is_some() {
            schedule(&mut state, &self.uploads, spool_id, Instant::now());
        }
        Ok(())
    }

    /// Drops a file's unsent bytes. An item shows the remote's bytes again. A new file loses its name and never uploads, and its open handles keep its bytes until they close, as after a remove. A node with no unsent bytes is [`VfsError::NotFound`].
    pub async fn discard_unsent(&self, node: NodeId) -> Result<(), VfsError> {
        let _changes = self.lock_changes().await?;
        let mut state = self.state.lock().unwrap();
        let Some((spool_id, _)) = self.unsent_spool(&state, node) else {
            return Err(VfsError::NotFound);
        };
        if matches!(state.nodes[&node].identity, Identity::New(_)) {
            // `unname` stops the try.
            unname(&mut state, &self.rules, &self.room, node);
        } else {
            stop_attempt(&mut state, spool_id);
            back_to_remote(&mut state, &self.room, node, spool_id);
        }
        Ok(())
    }

    /// What a spool file sends now, and where: `None` unless its node holds it, as an item or a new file, under a name that does not stay on this computer (decision 0005, choice 14).
    pub(super) fn sends(&self, state: &State, spool_id: SpoolId) -> Option<(Target, Place)> {
        let stays_local = self.spool.as_ref()?.stays_local;
        let (target, place) = sends_to(state, spool_id)?;
        (!stays_local(&place.name)).then_some((target, place))
    }

    /// The spool file of a node that has unsent bytes, and where they go.
    fn unsent_spool(&self, state: &State, node: NodeId) -> Option<(SpoolId, Place)> {
        let Bytes::Spooled(spool_id) = state.nodes.get(&node)?.bytes else {
            return None;
        };
        let (_, place) = self.sends(state, spool_id)?;
        Some((spool_id, place))
    }

    /// One try of one spool file.
    async fn upload(self: Arc<Self>, job: Job, base: Option<Revision>) {
        if job.check_remote_first {
            // An edit or a discard stops the check too, so it never downloads a large file for nothing. The next try checks again.
            let found = tokio::select! {
                biased;
                found = self.find_landed(&job, base.as_ref()) => found,
                _ = job.stop.notified() => return,
            };
            match found {
                Ok(Some((item, landed))) => {
                    self.confirm(&job, item, landed).await;
                    return;
                }
                Ok(None) => {
                    if let Some(spool) = self.state.lock().unwrap().spools.get_mut(&job.spool_id) {
                        spool.sent = Sent::ThisRun;
                    }
                }
                Err(error) => {
                    self.fail_try(&job, error);
                    return;
                }
            }
        }
        // The record that says a try may have reached the remote is on the disk before anything can land.
        let path = job.path.clone();
        let synced = match tokio::task::spawn_blocking(move || spool::sync_to_disk(&path)).await {
            Ok(result) => result.map_err(spool::map_io),
            Err(error) => Err(VfsError::Other(error.to_string())),
        };
        if let Err(error) = synced {
            self.fail_try(&job, error);
            return;
        }
        match &job.target {
            Target::Replace { id } => {
                // Choice 7: an item is only ever replaced on the revision its bytes came from.
                let Some(base) = base else {
                    self.fail_try(&job, VfsError::NotSupported);
                    return;
                };
                let progress = self.progress(&job);
                let sent =
                    self.provider
                        .replace_bytes(id, &job.path, &base, job.modified, &progress);
                match self.drive(&job, sent).await {
                    None => {}
                    Some(Ok(item)) => self.confirm(&job, item, Landed::Replaced).await,
                    Some(Err(VfsError::Conflict)) => self.conflict_copy(&job, id).await,
                    Some(Err(VfsError::NotFound)) => self.recreate(&job, id).await,
                    Some(Err(error)) => self.fail_try(&job, error),
                }
            }
            Target::Create { folder, name } => {
                if let Some(item) = self.send_new(&job, folder, name).await {
                    let landed = Landed::Created {
                        folder: folder.clone(),
                        gone: None,
                    };
                    self.confirm(&job, item, landed).await;
                }
            }
        }
    }

    /// Sends the spool file as a new item named `name` in `folder`. `None` when the try was stopped or failed; a failure is kept for the retry.
    async fn send_new(self: &Arc<Self>, job: &Job, folder: &ItemId, name: &str) -> Option<Item> {
        let progress = self.progress(job);
        let sent = self
            .provider
            .create_file(folder, name, &job.path, job.modified, &progress);
        match self.drive(job, sent).await? {
            Ok(item) => Some(item),
            Err(error) => {
                self.fail_try(job, error);
                None
            }
        }
    }

    /// Runs one byte call until it answers, the try is stopped (`None`), or its byte count stops growing (`TimedOut`). A stop drops the call and reports nothing. Every other end is reported to the watcher, as [`Vfs::call`] does.
    async fn drive(
        &self,
        job: &Job,
        work: impl std::future::Future<Output = Result<Item, VfsError>>,
    ) -> Option<Result<Item, VfsError>> {
        // Each call reports its own bytes from zero, and gets a full stall time from its start.
        if let Some(attempt) = self
            .state
            .lock()
            .unwrap()
            .spools
            .get_mut(&job.spool_id)
            .and_then(|spool| spool.attempt.as_mut())
        {
            attempt.sent = 0;
            attempt.moved = Instant::now();
        }
        let moved = || {
            let state = self.state.lock().unwrap();
            let spool = state.spools.get(&job.spool_id)?;
            Some(spool.attempt.as_ref()?.moved)
        };
        let stalled = async {
            // A spool file that went away mid-try has also stopped the try.
            while let Some(at) = moved() {
                tokio::time::sleep_until(at + self.options.upload_stall).await;
                if moved() == Some(at) {
                    return;
                }
            }
            std::future::pending::<()>().await
        };
        // An answer that is ready wins over a stop: the remote may already hold it.
        let result = tokio::select! {
            biased;
            result = work => result,
            _ = job.stop.notified() => return None,
            _ = stalled => Err(VfsError::TimedOut),
        };
        if let Some(watcher) = &self.watcher {
            watcher(result.as_ref().err());
        }
        Some(result)
    }

    /// Hears the provider's progress for this try. A byte count that grows moves the stall clock on.
    fn progress(self: &Arc<Self>, job: &Job) -> impl Fn(u64) + Send + Sync {
        let vfs = Arc::clone(self);
        let spool_id = job.spool_id;
        move |sent| {
            let mut state = vfs.state.lock().unwrap();
            let attempt = state
                .spools
                .get_mut(&spool_id)
                .and_then(|spool| spool.attempt.as_mut())
                .filter(|attempt| sent > attempt.sent);
            if let Some(attempt) = attempt {
                attempt.sent = sent;
                attempt.moved = Instant::now();
            }
        }
    }

    /// Where a second call of a replace (a conflict copy or a re-create) puts the file: the item's current place. `None` when the file no longer sends to this item.
    fn place_now(&self, job: &Job, id: &ItemId) -> Option<Place> {
        match self.sends(&self.state.lock().unwrap(), job.spool_id)? {
            (Target::Replace { id: now }, place) if &now == id => Some(place),
            _ => None,
        }
    }

    /// The remote changed the item since the spool's bytes left it. The local bytes become a new file beside it, and the node shows the remote's version (choice 7).
    async fn conflict_copy(self: &Arc<Self>, job: &Job, id: &ItemId) {
        let Some(place) = self.place_now(job, id) else {
            return;
        };
        let name = stamped_name(&place.name, "conflict", chrono::Local::now());
        let Some(copy) = self.send_new(job, &place.folder, &name).await else {
            return;
        };
        // The node and the listing hold the item's size and revision from before the remote's change. A failure here leaves them: they go stale with time.
        let (at, generation) = (Instant::now(), self.state.lock().unwrap().generation);
        let remote = self.call(self.provider.item(id)).await.ok().map(|item| {
            let asked = Asked {
                at,
                generation,
                answered: Instant::now(),
            };
            (asked, item)
        });
        let landed = Landed::Copied {
            folder: place.folder,
            remote,
        };
        self.confirm(job, copy, landed).await;
    }

    /// The remote no longer has the item. The local bytes go up as a new file under the item's current name.
    async fn recreate(self: &Arc<Self>, job: &Job, id: &ItemId) {
        let Some(place) = self.place_now(job, id) else {
            return;
        };
        if let Some(item) = self.send_new(job, &place.folder, &place.name).await {
            let landed = Landed::Created {
                folder: place.folder,
                gone: Some(id.clone()),
            };
            self.confirm(job, item, landed).await;
        }
    }

    /// The provider took the bytes. When the file still sends what the try began with, the node is tied to the item, and the spool file goes if these were its bytes.
    async fn confirm(self: &Arc<Self>, job: &Job, item: Item, landed: Landed) {
        let stray = {
            // A name change resolves a name, then acts on it, under this lock. Turning a new file into an item inside that would break the change.
            let _changes = self.changes.lock().await;
            let mut state = self.state.lock().unwrap();
            let current = self
                .sends(&state, job.spool_id)
                .filter(|(target, _)| *target == job.target)
                .map(|(_, place)| place);
            let sent_now = current.is_some() && state.spools[&job.spool_id].version == job.version;
            match (landed, current) {
                // The copy is on the remote whatever happened to the file since.
                (Landed::Copied { folder, remote }, current) => {
                    // A listing asked for after the create may hold the copy already.
                    change_listing(&mut state, &self.rules, &folder, |listed| {
                        listed.retain(|entry| entry.id != item.id);
                        listed.push(item.clone());
                    });
                    note_changed(&mut state, self.options.call_timeout, &folder);
                    // The node shows the remote's version from now on, so it takes the remote's size and revision.
                    if let (Some((asked, remote)), Some(place)) = (remote, current) {
                        let found = state.nodes.get_mut(&job.node).unwrap();
                        remember(&mut found.attributes, asked, remote.attributes.clone());
                        change_listing(&mut state, &self.rules, &place.folder, |listed| {
                            if let Some(entry) =
                                listed.iter_mut().find(|entry| entry.id == remote.id)
                            {
                                *entry = remote;
                            }
                        });
                    }
                    if sent_now {
                        back_to_remote(&mut state, &self.room, job.node, job.spool_id);
                    } else if let Some(spool) = state.spools.get_mut(&job.spool_id) {
                        spool.sent = Sent::Never;
                        let _ = save_record(&mut state, job.spool_id);
                    }
                    None
                }
                (Landed::Replaced, None) => None,
                (Landed::Created { folder, .. }, None) => Some(folder),
                (landed, Some(place)) => {
                    if let Landed::Created { gone, .. } = landed {
                        match &gone {
                            None => unlist(&mut state, &self.rules, job.node),
                            Some(gone) => {
                                state.by_item.remove(gone);
                            }
                        }
                        state.by_item.insert(item.id.clone(), job.node);
                        state.nodes.get_mut(&job.node).unwrap().identity = Identity::Item {
                            id: item.id.clone(),
                            place: Some(place.clone()),
                        };
                        // A listing asked for after the create may hold the item already.
                        change_listing(&mut state, &self.rules, &place.folder, |remote| {
                            remote.retain(|listed| {
                                listed.id != item.id && Some(&listed.id) != gone.as_ref()
                            });
                            remote.push(item.clone());
                        });
                    } else {
                        // Another client may have moved the item since this mount listed it, so only an entry still there is updated.
                        change_listing(&mut state, &self.rules, &place.folder, |remote| {
                            if let Some(listed) =
                                remote.iter_mut().find(|listed| listed.id == item.id)
                            {
                                *listed = item.clone();
                            }
                        });
                    }
                    note_changed(&mut state, self.options.call_timeout, &place.folder);
                    // The confirm is the newest fact about the item: a listing asked for during the upload may carry the old size and revision, and must not outrank it.
                    let now = Instant::now();
                    let asked = Asked {
                        at: now,
                        generation: state.generation,
                        answered: now,
                    };
                    let found = state.nodes.get_mut(&job.node).unwrap();
                    remember(&mut found.attributes, asked, item.attributes.clone());
                    if sent_now {
                        back_to_remote(&mut state, &self.room, job.node, job.spool_id);
                    } else {
                        // The remote now has the bytes this try sent, so the next replace must not take its own upload for a change.
                        let spool = state.spools.get_mut(&job.spool_id).unwrap();
                        spool.base = item.attributes.revision.clone();
                        spool.sent = Sent::Never;
                        let _ = save_record(&mut state, job.spool_id);
                    }
                    None
                }
            }
        };
        // A create that answered after its file was removed, renamed, or replaced leaves an item nobody shows. If this remove fails, it stays on the remote and shows in a later listing. Nothing shows the item, so no name change has to wait for this.
        let Some(folder) = stray else {
            return;
        };
        if self
            .call(self.provider.remove(&item.id, &folder))
            .await
            .is_ok()
        {
            let mut state = self.state.lock().unwrap();
            change_listing(&mut state, &self.rules, &folder, |remote| {
                remote.retain(|listed| listed.id != item.id)
            });
        }
    }

    /// The try failed. It is tried again after a wait that doubles with each failure in a row. Whatever stopped a try has already set the next one.
    fn fail_try(&self, job: &Job, error: VfsError) {
        let mut state = self.state.lock().unwrap();
        let Some(spool) = state.spools.get_mut(&job.spool_id) else {
            return;
        };
        if !matches!(&spool.attempt, Some(attempt) if !attempt.stopped) {
            return;
        }
        let tries = spool.failure.as_ref().map_or(0, |(_, tries)| *tries);
        spool.failure = Some((error, tries + 1));
        let wait = RETRY_START.saturating_mul(1 << tries.min(6)).min(RETRY_MAX);
        schedule(
            &mut state,
            &self.uploads,
            job.spool_id,
            Instant::now() + wait,
        );
    }
}

/// `{stem} ({word} {date} {time}){extension}`. The time has no colon: Finder shows one as `/`, and Windows forbids it.
pub(super) fn stamped_name(name: &str, word: &str, at: chrono::DateTime<chrono::Local>) -> String {
    let (stem, extension) = crate::names::split_extension(name);
    let stamp = at.format("%Y-%m-%d %H-%M-%S");
    format!("{stem} ({word} {stamp}){extension}")
}

/// Whether `name` is `original` with a `word` stamp from [`stamped_name`].
fn is_stamped(name: &str, original: &str, word: &str) -> bool {
    let (stem, extension) = crate::names::split_extension(original);
    let (prefix, suffix) = (format!("{stem} ({word} "), format!("){extension}"));
    name.len() >= prefix.len() + suffix.len()
        && name.starts_with(&prefix)
        && name.ends_with(&suffix)
}

impl Vfs {
    /// Whether an earlier run's try of this adopted file already landed, and what it made. `Ok(None)` means the try should send.
    async fn find_landed(
        &self,
        job: &Job,
        base: Option<&Revision>,
    ) -> Result<Option<(Item, Landed)>, VfsError> {
        let id = match &job.target {
            Target::Create { folder, name } => {
                let found = self.find_sent(job, folder, |listed| listed == name).await?;
                let landed = || Landed::Created {
                    folder: folder.clone(),
                    gone: None,
                };
                return Ok(found.map(|item| (item, landed())));
            }
            Target::Replace { id } => id,
        };
        let Some(place) = self.place_now(job, id) else {
            return Ok(None);
        };
        let (at, generation) = (Instant::now(), self.state.lock().unwrap().generation);
        let remote = match self.call(self.provider.item(id)).await {
            Ok(remote) => remote,
            // The item is gone, and the try may have created it again under its name.
            Err(VfsError::NotFound) => {
                let found = self
                    .find_sent(job, &place.folder, |listed| listed == place.name)
                    .await?;
                let landed = || Landed::Created {
                    folder: place.folder.clone(),
                    gone: Some(id.clone()),
                };
                return Ok(found.map(|item| (item, landed())));
            }
            Err(error) => return Err(error),
        };
        if remote.attributes.revision.as_ref() == base {
            return Ok(None);
        }
        if self.holds_bytes(&remote, job).await? {
            return Ok(Some((remote, Landed::Replaced)));
        }
        // Another client changed the item, and the try may have made the conflict copy.
        let found = self
            .find_sent(job, &place.folder, |listed| {
                is_stamped(listed, &place.name, "conflict")
            })
            .await?;
        let asked = Asked {
            at,
            generation,
            answered: Instant::now(),
        };
        let landed = Landed::Copied {
            folder: place.folder.clone(),
            remote: Some((asked, remote)),
        };
        Ok(found.map(|copy| (copy, landed)))
    }

    /// A file in `folder` whose name `named` accepts and that holds this try's bytes. A folder that is gone holds none.
    async fn find_sent(
        &self,
        job: &Job,
        folder: &ItemId,
        named: impl Fn(&str) -> bool,
    ) -> Result<Option<Item>, VfsError> {
        // The cached listing serves a mount that adopts many files from one folder.
        let listing = match self.listing(folder).await {
            Ok(listing) => listing,
            Err(VfsError::NotFound) => return Ok(None),
            Err(error) => return Err(error),
        };
        for item in listing.remote.iter() {
            if named(&item.name) && self.holds_bytes(item, job).await? {
                return Ok(Some(item.clone()));
            }
        }
        Ok(None)
    }

    /// The item is a file of this try's length and modified time, and its bytes are the spool file's. The time counts in whole seconds, which Drive and SFTP keep.
    async fn holds_bytes(&self, item: &Item, job: &Job) -> Result<bool, VfsError> {
        use super::record::whole_seconds;
        if item.attributes.kind != crate::Kind::File
            || item.attributes.size != job.size
            || whole_seconds(item.attributes.modified) != whole_seconds(job.modified)
        {
            return Ok(false);
        }
        let mut offset = 0;
        while offset < job.size {
            let length = u64::from(PIECE).min(job.size - offset) as u32;
            let remote = self
                .call(self.provider.read(&item.id, offset, length))
                .await?;
            let path = job.path.clone();
            let local = tokio::task::spawn_blocking(move || spool::read_at(&path, offset, length))
                .await
                .map_err(|error| VfsError::Other(error.to_string()))?
                .map_err(spool::map_io)?;
            if remote != local || remote.len() != length as usize {
                return Ok(false);
            }
            offset += u64::from(length);
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::stamped_name;
    use chrono::TimeZone;

    #[test]
    fn a_conflict_copy_names_the_stem_and_keeps_the_extension() {
        let naive = chrono::NaiveDate::from_ymd_opt(2026, 11, 19)
            .unwrap()
            .and_hms_opt(14, 3, 27)
            .unwrap();
        let at = chrono::Local.from_local_datetime(&naive).single().unwrap();
        assert_eq!(
            stamped_name("report.docx", "conflict", at),
            "report (conflict 2026-11-19 14-03-27).docx"
        );
        assert_eq!(
            stamped_name(".bashrc", "conflict", at),
            ".bashrc (conflict 2026-11-19 14-03-27)"
        );
        assert_eq!(
            stamped_name("a.tar.gz", "conflict", at),
            "a.tar (conflict 2026-11-19 14-03-27).gz"
        );
        assert_eq!(
            stamped_name("report.docx", "recovered", at),
            "report (recovered 2026-11-19 14-03-27).docx"
        );
    }
}
