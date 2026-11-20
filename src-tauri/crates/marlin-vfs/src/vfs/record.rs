use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::time::Instant;

use crate::spool;
use crate::{escape, ItemId, NameRules, Revision};

use super::upload::{self, Target};
use super::{list, Bytes, Identity, Node, NodeId, Place, Sent, SpoolFile, SpoolId, State};

const VERSION: u32 = 1;

/// What the core knows about one spool file, so the next mount can place it (decision 0005, choice 10).
#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub(super) struct Record {
    version: u32,
    sends: Option<Sends>,
    base: Option<String>,
    closed: Option<Closed>,
    /// A byte call may have reached the remote.
    tried: bool,
}

/// Where the spool file's bytes go. `None` on [`Record`] means they go nowhere.
#[derive(Serialize, Deserialize, Clone, PartialEq)]
enum Sends {
    Item {
        id: String,
        folder: String,
        name: String,
    },
    New {
        folder: String,
        name: String,
    },
}

/// Present once every writer has closed and the bytes still go somewhere.
#[derive(Serialize, Deserialize, Clone, PartialEq)]
struct Closed {
    size: u64,
    disk_modified: Time,
    modified: Time,
}

/// A [`SystemTime`] relative to the unix epoch. `secs` is negative for a time before it.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq)]
struct Time {
    secs: i64,
    nanos: u32,
}

impl Time {
    fn from_system(time: SystemTime) -> Self {
        let nanos = match time.duration_since(UNIX_EPOCH) {
            Ok(after) => after.as_nanos() as i128,
            Err(before) => -(before.duration().as_nanos() as i128),
        };
        Self {
            secs: nanos.div_euclid(1_000_000_000) as i64,
            nanos: nanos.rem_euclid(1_000_000_000) as u32,
        }
    }

    fn to_system(self) -> SystemTime {
        let whole = match u64::try_from(self.secs) {
            Ok(secs) => UNIX_EPOCH + Duration::from_secs(secs),
            Err(_) => UNIX_EPOCH - Duration::from_secs(self.secs.unsigned_abs()),
        };
        whole + Duration::from_nanos(u64::from(self.nanos))
    }
}

/// Whole seconds since the epoch, counted down for a time before it, as a record stores them.
pub(super) fn whole_seconds(time: SystemTime) -> i64 {
    Time::from_system(time).secs
}

/// Writes the record of spool file `id` when what it would hold differs from what was last written.
///
/// A failed write leaves the remembered record alone, so the next call tries again. The write is not synced: [`super::Vfs::fsync`] and the last close put it on the disk.
pub(super) fn save_record(state: &mut State, id: SpoolId) -> io::Result<()> {
    let record = build_record(state, id)?;
    let spool = state.spools.get_mut(&id).unwrap();
    if spool.written.as_ref() == Some(&record) {
        return Ok(());
    }
    spool::write_record(&spool.path, &serde_json::to_vec(&record)?)?;
    spool.written = Some(record);
    Ok(())
}

fn build_record(state: &State, id: SpoolId) -> io::Result<Record> {
    let spool = &state.spools[&id];
    let sends = match upload::sends_to(state, id) {
        Some((Target::Replace { id }, place)) => Some(Sends::Item {
            id: id.0,
            folder: place.folder.0,
            name: place.name,
        }),
        Some((Target::Create { folder, name }, _)) => Some(Sends::New {
            folder: folder.0,
            name,
        }),
        None => None,
    };
    // `sends` is only `Some` while the node holds this file. A write after this stat changes the length or the modified time, which is how the next mount tells this record from the file.
    let closed = if sends.is_some() && state.nodes[&spool.node].writers == 0 {
        let metadata = std::fs::metadata(&spool.path)?;
        Some(Closed {
            size: metadata.len(),
            disk_modified: Time::from_system(metadata.modified()?),
            modified: Time::from_system(spool.modified),
        })
    } else {
        None
    };
    Ok(Record {
        version: VERSION,
        sends,
        base: spool.base.as_ref().map(|revision| revision.0.clone()),
        closed,
        tried: spool.sent != Sent::Never,
    })
}

/// Reads `folder` and puts back what an earlier run left there. Returns the spool files that could not be placed, and the next spool file's number.
pub(super) fn recover(
    state: &mut State,
    folder: &Path,
    stays_local: fn(&str) -> bool,
    rules: &NameRules,
) -> io::Result<(Vec<PathBuf>, u64)> {
    let mut files = BTreeMap::new();
    let mut records = BTreeMap::new();
    let mut half_written = Vec::new();
    for entry in std::fs::read_dir(folder)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if let Some(Ok(number)) = name.strip_suffix(".record.new").map(str::parse::<u64>) {
            half_written.push((number, path));
        } else if let Some(Ok(number)) = name.strip_suffix(".record").map(str::parse::<u64>) {
            records.insert(number, path);
        } else if let Ok(number) = name.parse::<u64>() {
            files.insert(number, path);
        }
    }
    let next = files
        .keys()
        .chain(records.keys())
        .chain(half_written.iter().map(|(number, _)| number))
        .max()
        .map_or(1, |number| number + 1);
    // A crash mid-write left it, and the record it was replacing stands.
    for (_, path) in half_written {
        remove_file(&path)?;
    }
    for (number, path) in &records {
        if !files.contains_key(number) {
            remove_file(path)?;
        }
    }
    let mut unplaced = Vec::new();
    let now = Instant::now();
    for (number, path) in files {
        let metadata = std::fs::metadata(&path)?;
        let len = metadata.len();
        let disk_modified = Time::from_system(metadata.modified()?);
        let Some(record) = records.get(&number).and_then(|record| read_record(record)) else {
            // It takes room on the disk, so it counts against the limit.
            state.spool_used += len;
            unplaced.push(path);
            continue;
        };
        let place = match &record.sends {
            Some(Sends::Item { folder, name, .. } | Sends::New { folder, name })
                if !stays_local(name) =>
            {
                Place {
                    folder: ItemId(folder.clone()),
                    name: name.clone(),
                }
            }
            // A fetch that never published, a removed file, or a name that stays on this computer.
            _ => {
                remove_file(&path)?;
                remove_file(&spool::record_path(&path))?;
                continue;
            }
        };
        let closed = record
            .closed
            .as_ref()
            .filter(|closed| closed.size == len && closed.disk_modified == disk_modified)
            .map(|closed| closed.modified.to_system());
        let earlier = match record.tried {
            true => Sent::EarlierRun,
            false => Sent::Never,
        };
        let (item, place, base, modified, sent) = match (closed, &record.sends) {
            (Some(modified), Some(Sends::Item { id, .. })) => (
                Some(ItemId(id.clone())),
                place,
                record.base.clone().map(Revision),
                modified,
                earlier,
            ),
            (Some(modified), _) => (None, place, None, modified, earlier),
            // Still open, or written after its record: it may hold half an edit, so it never goes over the original.
            (None, _) => {
                let name = upload::stamped_name(&place.name, "recovered", chrono::Local::now());
                let place = Place {
                    folder: place.folder,
                    name,
                };
                (None, place, None, disk_modified.to_system(), Sent::Never)
            }
        };
        let id = SpoolId(number);
        let node = NodeId(state.next_node);
        state.next_node += 1;
        state.spools.insert(
            id,
            SpoolFile {
                path,
                size: len,
                reserved: 0,
                modified,
                edits: Arc::new(tokio::sync::Mutex::new(())),
                node,
                base,
                version: 0,
                editing: false,
                due: now,
                attempt: None,
                failure: None,
                sent,
                written: Some(record),
            },
        );
        state.spool_used += len;
        // No settle time: its writers closed before the crash.
        state.queue.insert((now, id));
        // A new file gets its identity from `list` below.
        let identity = match &item {
            Some(item) => Identity::Item {
                id: item.clone(),
                place: Some(place.clone()),
            },
            None => Identity::Unnamed,
        };
        state.nodes.insert(
            node,
            Node {
                identity,
                lookups: 0,
                handles: 0,
                writers: 0,
                attributes: None,
                bytes: Bytes::Spooled(id),
            },
        );
        match item {
            Some(item) => {
                state.by_item.insert(item, node);
            }
            // `list` takes the name the OS sends, and the record holds the remote name.
            None => list(
                state,
                rules,
                node,
                &place.folder,
                &escape(&place.name, rules),
            ),
        }
        // A recovered copy's new name goes on the disk, so a second crash does not make a second copy.
        let _ = save_record(state, id);
    }
    Ok((unplaced, next))
}

fn read_record(path: &Path) -> Option<Record> {
    let bytes = std::fs::read(path).ok()?;
    let record: Record = serde_json::from_slice(&bytes).ok()?;
    (record.version == VERSION).then_some(record)
}

fn remove_file(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::Time;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn a_time_keeps_its_value_on_both_sides_of_the_epoch() {
        for time in [
            UNIX_EPOCH + Duration::new(1_700_000_000, 123),
            UNIX_EPOCH,
            UNIX_EPOCH - Duration::from_secs(2),
            UNIX_EPOCH - Duration::from_millis(1_500),
        ] {
            assert_eq!(Time::from_system(time).to_system(), time);
        }
        // A time before the epoch counts its seconds down from the next lower whole second, so 1.5 s before is -2 s plus 0.5 s.
        assert_eq!(
            Time::from_system(UNIX_EPOCH - Duration::from_millis(1_500)).secs,
            -2
        );
    }
}
