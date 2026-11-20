use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::sync::Arc;
use std::time::SystemTime;

use fuser::{
    BsdFileFlags, Errno, FileAttr, FileHandle, FileType, Filesystem, FopenFlags, Generation,
    INodeNo, InitFlags, KernelConfig, LockOwner, OpenFlags, RenameFlags, ReplyAttr, ReplyCreate,
    ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyOpen, ReplyStatfs, ReplyWrite, Request,
    TimeOrNow, WriteFlags,
};
use marlin_vfs::{Attributes, HandleId, IfExists, Kind, NodeId, Vfs};

use crate::Shown;

/// The core keeps no parent for a node. This is the value libfuse sends (`FUSE_UNKNOWN_INO`) when a filesystem does not know an entry's inode.
const UNKNOWN_INODE: INodeNo = INodeNo(0xffff_ffff);

/// Serves one `Vfs` to the kernel. A callback that may reach the provider answers from a tokio task, so a slow call never holds up the `fuser` thread. `forget`, `opendir`, `readdir` past offset 0, and the two releases answer in place. They touch the core's own state, and the last writer's release also writes the file's small spool record, with no sync. `flush` is left out: every write is in the spool before it answers, so a close has nothing to flush, and after the first `ENOSYS` the kernel stops sending it. `release` starts the settle time. `mknod` is left out because the kernel uses `create` for a regular file. `fsyncdir`, `fallocate`, `copy_file_range`, `lseek`, `ioctl`, the xattr calls, and `getlk`/`setlk` are left out. `link` and `symlink` stay at `fuser`'s `EPERM`.
pub(crate) struct Fs {
    pub(crate) vfs: Arc<Vfs>,
    pub(crate) runtime: tokio::runtime::Handle,
    pub(crate) shown: Shown,
    /// Whether an open may keep the kernel's cached bytes of the file. Set in `init`.
    pub(crate) keep_cache: bool,
}

impl Fs {
    fn fopen_flags(&self) -> FopenFlags {
        if self.keep_cache {
            FopenFlags::FOPEN_KEEP_CACHE
        } else {
            FopenFlags::empty()
        }
    }
}

impl Filesystem for Fs {
    fn init(&mut self, _req: &Request, config: &mut KernelConfig) -> std::io::Result<()> {
        // fuser asks the kernel for async reads, big writes, and the max page count only.
        // FOPEN_KEEP_CACHE keeps a file's pages across opens. Those pages stay safe only
        // with FUSE_AUTO_INVAL_DATA, which drops them when the modified time or the size
        // changes. A kernel that cannot do that must not be asked to keep pages.
        self.keep_cache = config
            .add_capabilities(InitFlags::FUSE_AUTO_INVAL_DATA)
            .is_ok();
        Ok(())
    }

    fn lookup(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        let name = match crate::utf8_name(name.as_bytes(), crate::Name::Existing) {
            Ok(name) => name,
            Err(errno) => return reply.error(Errno::from_i32(errno)),
        };
        let vfs = Arc::clone(&self.vfs);
        let shown = self.shown;
        self.runtime.spawn(async move {
            match vfs.lookup(NodeId(parent.0), &name).await {
                Ok(entry) => {
                    let attr = shown.file_attr(entry.node, &entry.attributes);
                    reply.entry(&crate::TTL, &attr, Generation(0));
                }
                Err(error) => reply.error(Errno::from_i32(crate::errno(&error))),
            }
        });
    }

    fn forget(&self, _req: &Request, ino: INodeNo, nlookup: u64) {
        self.vfs.forget(NodeId(ino.0), nlookup);
    }

    fn getattr(&self, _req: &Request, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
        let vfs = Arc::clone(&self.vfs);
        let shown = self.shown;
        self.runtime.spawn(async move {
            let node = NodeId(ino.0);
            match vfs.attributes(node).await {
                Ok(attributes) => {
                    let attr = shown.file_attr(node, &attributes);
                    reply.attr(&crate::TTL, &attr);
                }
                Err(error) => reply.error(Errno::from_i32(crate::errno(&error))),
            }
        });
    }

    fn setattr(
        &self,
        _req: &Request,
        ino: INodeNo,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<TimeOrNow>,
        mtime: Option<TimeOrNow>,
        _ctime: Option<SystemTime>,
        _fh: Option<FileHandle>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<BsdFileFlags>,
        reply: ReplyAttr,
    ) {
        // An open with O_TRUNC arrives as a size of 0. The core cuts a clean file to zero
        // without reading it. The adapter does not ask for FUSE_ATOMIC_O_TRUNC.
        let mtime = mtime.map(|time| match time {
            TimeOrNow::SpecificTime(time) => time,
            TimeOrNow::Now => SystemTime::now(),
        });
        let vfs = Arc::clone(&self.vfs);
        let shown = self.shown;
        self.runtime.spawn(async move {
            let node = NodeId(ino.0);
            match crate::set_attributes(&vfs, node, size, mtime).await {
                Ok(attributes) => {
                    let attr = shown.file_attr(node, &attributes);
                    reply.attr(&crate::TTL, &attr);
                }
                Err(error) => reply.error(Errno::from_i32(crate::errno(&error))),
            }
        });
    }

    fn open(&self, _req: &Request, ino: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
        let access = crate::open_access(flags.0);
        let vfs = Arc::clone(&self.vfs);
        let flags = self.fopen_flags();
        self.runtime.spawn(async move {
            match vfs.open(NodeId(ino.0), access).await {
                Ok(handle) => reply.opened(FileHandle(handle.0), flags),
                Err(error) => reply.error(Errno::from_i32(crate::errno(&error))),
            }
        });
    }

    fn read(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyData,
    ) {
        let vfs = Arc::clone(&self.vfs);
        self.runtime.spawn(async move {
            match vfs.read(HandleId(fh.0), offset, size).await {
                Ok(bytes) => reply.data(&bytes),
                Err(error) => reply.error(Errno::from_i32(crate::errno(&error))),
            }
        });
    }

    fn write(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        data: &[u8],
        _write_flags: WriteFlags,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyWrite,
    ) {
        // Ignore O_APPEND in `_flags`: the kernel already put `offset` at the end of the file, from the size it holds, under the inode lock. There is no write-back cache.
        let len = data.len() as u32;
        let data = data.to_vec();
        let vfs = Arc::clone(&self.vfs);
        self.runtime.spawn(async move {
            match vfs.write(HandleId(fh.0), offset, data).await {
                Ok(()) => reply.written(len),
                Err(error) => reply.error(Errno::from_i32(crate::errno(&error))),
            }
        });
    }

    fn release(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        self.vfs.release(HandleId(fh.0));
        reply.ok();
    }

    fn fsync(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        _datasync: bool,
        reply: ReplyEmpty,
    ) {
        let vfs = Arc::clone(&self.vfs);
        self.runtime.spawn(async move {
            match vfs.fsync(NodeId(ino.0)).await {
                Ok(()) => reply.ok(),
                Err(error) => reply.error(Errno::from_i32(crate::errno(&error))),
            }
        });
    }

    fn opendir(&self, _req: &Request, ino: INodeNo, _flags: OpenFlags, reply: ReplyOpen) {
        match self.vfs.open_folder(NodeId(ino.0)) {
            Ok(handle) => reply.opened(FileHandle(handle.0), FopenFlags::empty()),
            Err(error) => reply.error(Errno::from_i32(crate::errno(&error))),
        }
    }

    fn readdir(
        &self,
        _req: &Request,
        ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        reply: ReplyDirectory,
    ) {
        // The listing is taken at every read from offset 0: the first one, and a rewind (`rewinddir`) on the same handle.
        if offset == 0 {
            let vfs = Arc::clone(&self.vfs);
            self.runtime.spawn(async move {
                if let Err(error) = vfs.rewind_folder(HandleId(fh.0)).await {
                    reply.error(Errno::from_i32(crate::errno(&error)));
                    return;
                }
                fill_dir(&vfs, ino, fh, 0, reply);
            });
            return;
        }
        fill_dir(&self.vfs, ino, fh, offset, reply);
    }

    fn releasedir(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        _flags: OpenFlags,
        reply: ReplyEmpty,
    ) {
        self.vfs.release(HandleId(fh.0));
        reply.ok();
    }

    fn mkdir(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        let name = match crate::utf8_name(name.as_bytes(), crate::Name::New) {
            Ok(name) => name,
            Err(errno) => return reply.error(Errno::from_i32(errno)),
        };
        let vfs = Arc::clone(&self.vfs);
        let shown = self.shown;
        self.runtime.spawn(async move {
            match vfs.create_folder(NodeId(parent.0), &name).await {
                Ok(entry) => {
                    let attr = shown.file_attr(entry.node, &entry.attributes);
                    reply.entry(&crate::TTL, &attr, Generation(0));
                }
                Err(error) => reply.error(Errno::from_i32(crate::errno(&error))),
            }
        });
    }

    fn unlink(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        let name = match crate::utf8_name(name.as_bytes(), crate::Name::Existing) {
            Ok(name) => name,
            Err(errno) => return reply.error(Errno::from_i32(errno)),
        };
        let vfs = Arc::clone(&self.vfs);
        self.runtime.spawn(async move {
            match vfs.remove(NodeId(parent.0), &name, Kind::File).await {
                Ok(()) => reply.ok(),
                Err(error) => reply.error(Errno::from_i32(crate::errno(&error))),
            }
        });
    }

    fn rmdir(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        let name = match crate::utf8_name(name.as_bytes(), crate::Name::Existing) {
            Ok(name) => name,
            Err(errno) => return reply.error(Errno::from_i32(errno)),
        };
        let vfs = Arc::clone(&self.vfs);
        self.runtime.spawn(async move {
            match vfs.remove(NodeId(parent.0), &name, Kind::Folder).await {
                Ok(()) => reply.ok(),
                Err(error) => reply.error(Errno::from_i32(crate::errno(&error))),
            }
        });
    }

    fn rename(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        newparent: INodeNo,
        newname: &OsStr,
        flags: RenameFlags,
        reply: ReplyEmpty,
    ) {
        let if_exists = match if_exists(flags) {
            Ok(if_exists) => if_exists,
            Err(errno) => return reply.error(errno),
        };
        // A folder moved into itself never reaches here. The kernel refuses it with EINVAL from its own dentry tree.
        let name = match crate::utf8_name(name.as_bytes(), crate::Name::Existing) {
            Ok(name) => name,
            Err(errno) => return reply.error(Errno::from_i32(errno)),
        };
        let newname = match crate::utf8_name(newname.as_bytes(), crate::Name::New) {
            Ok(name) => name,
            Err(errno) => return reply.error(Errno::from_i32(errno)),
        };
        let vfs = Arc::clone(&self.vfs);
        self.runtime.spawn(async move {
            match vfs
                .rename(
                    NodeId(parent.0),
                    &name,
                    NodeId(newparent.0),
                    &newname,
                    if_exists,
                )
                .await
            {
                Ok(()) => reply.ok(),
                Err(error) => reply.error(Errno::from_i32(crate::errno(&error))),
            }
        });
    }

    fn create(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        flags: i32,
        reply: ReplyCreate,
    ) {
        let name = match crate::utf8_name(name.as_bytes(), crate::Name::New) {
            Ok(name) => name,
            Err(errno) => return reply.error(Errno::from_i32(errno)),
        };
        let access = crate::open_access(flags);
        let vfs = Arc::clone(&self.vfs);
        let shown = self.shown;
        let flags = self.fopen_flags();
        self.runtime.spawn(async move {
            match vfs.create(NodeId(parent.0), &name, access).await {
                Ok((entry, handle)) => {
                    let attr = shown.file_attr(entry.node, &entry.attributes);
                    reply.created(
                        &crate::TTL,
                        &attr,
                        Generation(0),
                        FileHandle(handle.0),
                        flags,
                    );
                }
                Err(error) => reply.error(Errno::from_i32(crate::errno(&error))),
            }
        });
    }

    fn statfs(&self, _req: &Request, _ino: INodeNo, reply: ReplyStatfs) {
        let vfs = Arc::clone(&self.vfs);
        self.runtime.spawn(async move {
            let block = u64::from(crate::BLOCK_SIZE);
            match vfs.space().await {
                Ok(space) => reply.statfs(
                    space.total / block,
                    space.free / block,
                    space.free / block,
                    0,
                    0,
                    crate::BLOCK_SIZE,
                    crate::NAME_MAX as u32,
                    crate::BLOCK_SIZE,
                ),
                Err(error) => reply.error(Errno::from_i32(crate::errno(&error))),
            }
        });
    }
}

/// What a rename does when the new name is taken. `RENAME_EXCHANGE` and `RENAME_WHITEOUT` are `EINVAL`: Linux file systems answer that for a rename flag they do not support, and the FUSE kernel code does the same when a server has no `RENAME2`.
fn if_exists(flags: RenameFlags) -> Result<IfExists, Errno> {
    if flags.intersects(RenameFlags::RENAME_EXCHANGE | RenameFlags::RENAME_WHITEOUT) {
        return Err(Errno::EINVAL);
    }
    if flags.contains(RenameFlags::RENAME_NOREPLACE) {
        Ok(IfExists::Refuse)
    } else {
        Ok(IfExists::Replace)
    }
}

/// Fills one readdir reply from the frozen listing. Offset 0 is `.`, offset 1 is `..`, and listing entry `i` is offset `i + 2`. The offset sent with an entry is where the next call resumes. An entry that does not fit is not sent.
fn fill_dir(
    vfs: &Vfs,
    parent: INodeNo,
    fh: FileHandle,
    mut offset: u64,
    mut reply: ReplyDirectory,
) {
    if offset == 0 {
        if reply.add(parent, 1, FileType::Directory, ".") {
            return reply.ok();
        }
        offset = 1;
    }
    if offset == 1 {
        if reply.add(UNKNOWN_INODE, 2, FileType::Directory, "..") {
            return reply.ok();
        }
        offset = 2;
    }
    loop {
        let page = match vfs.read_folder(HandleId(fh.0), (offset - 2) as usize, crate::PAGE) {
            Ok(page) => page,
            Err(error) => return reply.error(Errno::from_i32(crate::errno(&error))),
        };
        if page.is_empty() {
            return reply.ok();
        }
        for entry in page {
            offset += 1;
            if entry.name.len() > crate::NAME_MAX {
                continue;
            }
            let kind = match entry.attributes.kind {
                Kind::Folder => FileType::Directory,
                Kind::File => FileType::RegularFile,
            };
            if reply.add(INodeNo(entry.node.0), offset, kind, entry.name) {
                return reply.ok();
            }
        }
    }
}

impl Shown {
    fn file_attr(self, node: NodeId, attributes: &Attributes) -> FileAttr {
        let (perm, nlink) = crate::mode_and_links(attributes.kind, self.writable);
        let kind = match attributes.kind {
            Kind::Folder => FileType::Directory,
            Kind::File => FileType::RegularFile,
        };
        FileAttr {
            ino: INodeNo(node.0),
            size: attributes.size,
            blocks: attributes.size.div_ceil(512),
            atime: attributes.modified,
            mtime: attributes.modified,
            ctime: attributes.modified,
            crtime: attributes.modified,
            kind,
            perm,
            nlink,
            uid: self.uid,
            gid: self.gid,
            rdev: 0,
            blksize: crate::BLOCK_SIZE,
            flags: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;

    fn attributes(kind: Kind, size: u64) -> Attributes {
        Attributes {
            kind,
            size,
            modified: UNIX_EPOCH + Duration::from_secs(9),
            revision: None,
        }
    }

    fn shown(writable: bool) -> Shown {
        Shown {
            uid: 11,
            gid: 22,
            writable,
        }
    }

    #[test]
    fn file_attr_for_a_file() {
        let modified = UNIX_EPOCH + Duration::from_secs(9);
        let attr = shown(false).file_attr(NodeId(7), &attributes(Kind::File, 1000));
        assert_eq!(attr.kind, FileType::RegularFile);
        assert_eq!(attr.perm, 0o400);
        assert_eq!(attr.nlink, 1);
        assert_eq!(attr.size, 1000);
        assert_eq!(attr.blocks, 2);
        assert_eq!(attr.atime, modified);
        assert_eq!(attr.mtime, modified);
        assert_eq!(attr.ctime, modified);
        assert_eq!(attr.crtime, modified);
        assert_eq!(attr.uid, 11);
        assert_eq!(attr.gid, 22);
        assert_eq!(
            shown(true)
                .file_attr(NodeId(7), &attributes(Kind::File, 1000))
                .perm,
            0o600
        );
    }

    #[test]
    fn file_attr_for_a_folder() {
        let modified = UNIX_EPOCH + Duration::from_secs(9);
        let attr = shown(false).file_attr(NodeId(4), &attributes(Kind::Folder, 40));
        assert_eq!(attr.kind, FileType::Directory);
        assert_eq!(attr.perm, 0o500);
        assert_eq!(attr.nlink, 2);
        assert_eq!(attr.size, 40);
        assert_eq!(attr.blocks, 1);
        assert_eq!(attr.atime, modified);
        assert_eq!(attr.mtime, modified);
        assert_eq!(attr.ctime, modified);
        assert_eq!(attr.crtime, modified);
        assert_eq!(attr.uid, 11);
        assert_eq!(attr.gid, 22);
        assert_eq!(
            shown(true)
                .file_attr(NodeId(4), &attributes(Kind::Folder, 40))
                .perm,
            0o700
        );
    }

    #[test]
    fn rename_flags_map_to_what_a_taken_name_does() {
        assert_eq!(if_exists(RenameFlags::empty()), Ok(IfExists::Replace));
        assert_eq!(
            if_exists(RenameFlags::RENAME_NOREPLACE),
            Ok(IfExists::Refuse)
        );
        assert_eq!(if_exists(RenameFlags::RENAME_EXCHANGE), Err(Errno::EINVAL));
        assert_eq!(if_exists(RenameFlags::RENAME_WHITEOUT), Err(Errno::EINVAL));
    }
}
