use std::ffi::OsStr;
use std::sync::Arc;
use std::time::Duration;

use fuser::{
    Errno, FileAttr, FileHandle, FileType, Filesystem, FopenFlags, Generation, INodeNo, LockOwner,
    OpenAccMode, OpenFlags, ReplyAttr, ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry,
    ReplyOpen, ReplyStatfs, Request,
};
use marlin_vfs::{Access, Attributes, HandleId, Kind, NodeId, Vfs, VfsError};

/// How long the kernel may reuse a name and its attributes without asking again.
const TTL: Duration = Duration::from_secs(1);

const BLOCK_SIZE: u32 = 4096;

/// The longest name Linux apps expect, in bytes. A longer name is not listed and cannot be looked up.
const NAME_MAX: usize = 255;

/// One page of a folder listing. A bigger folder is read in pages, so one readdir never clones the whole listing.
const PAGE: usize = 256;

/// The core keeps no parent for a node. This is the value libfuse sends (`FUSE_UNKNOWN_INO`) when a filesystem does not know an entry's inode.
const UNKNOWN_INODE: INodeNo = INodeNo(0xffff_ffff);

/// Serves one `Vfs` to the kernel. A callback that may reach the provider answers from a tokio task, so a slow call never holds up the `fuser` thread. `forget`, `readdir`, and the two releases only touch the core's own state and answer in place.
pub(crate) struct Fs {
    pub(crate) vfs: Arc<Vfs>,
    pub(crate) runtime: tokio::runtime::Handle,
    pub(crate) uid: u32,
    pub(crate) gid: u32,
}

impl Filesystem for Fs {
    fn lookup(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        let Some(name) = name.to_str() else {
            return reply.error(Errno::ENOENT);
        };
        if name.len() > NAME_MAX {
            return reply.error(Errno::ENAMETOOLONG);
        }
        let name = name.to_owned();
        let vfs = Arc::clone(&self.vfs);
        let uid = self.uid;
        let gid = self.gid;
        self.runtime.spawn(async move {
            match vfs.lookup(NodeId(parent.0), &name).await {
                Ok(entry) => {
                    let attr = file_attr(entry.node, &entry.attributes, uid, gid);
                    reply.entry(&TTL, &attr, Generation(0));
                }
                Err(error) => reply.error(errno(&error)),
            }
        });
    }

    fn forget(&self, _req: &Request, ino: INodeNo, nlookup: u64) {
        self.vfs.forget(NodeId(ino.0), nlookup);
    }

    fn getattr(&self, _req: &Request, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
        let vfs = Arc::clone(&self.vfs);
        let uid = self.uid;
        let gid = self.gid;
        self.runtime.spawn(async move {
            let node = NodeId(ino.0);
            match vfs.attributes(node).await {
                Ok(attributes) => {
                    let attr = file_attr(node, &attributes, uid, gid);
                    reply.attr(&TTL, &attr);
                }
                Err(error) => reply.error(errno(&error)),
            }
        });
    }

    fn open(&self, _req: &Request, ino: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
        let access = if flags.acc_mode() == OpenAccMode::O_RDONLY {
            Access::Read
        } else {
            Access::Write
        };
        let vfs = Arc::clone(&self.vfs);
        self.runtime.spawn(async move {
            match vfs.open(NodeId(ino.0), access).await {
                Ok(handle) => reply.opened(FileHandle(handle.0), FopenFlags::empty()),
                Err(error) => reply.error(errno(&error)),
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
                Err(error) => reply.error(errno(&error)),
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

    fn opendir(&self, _req: &Request, ino: INodeNo, _flags: OpenFlags, reply: ReplyOpen) {
        let vfs = Arc::clone(&self.vfs);
        self.runtime.spawn(async move {
            match vfs.open_folder(NodeId(ino.0)).await {
                Ok(handle) => reply.opened(FileHandle(handle.0), FopenFlags::empty()),
                Err(error) => reply.error(errno(&error)),
            }
        });
    }

    fn readdir(
        &self,
        _req: &Request,
        ino: INodeNo,
        fh: FileHandle,
        mut offset: u64,
        mut reply: ReplyDirectory,
    ) {
        // Offset 0 is `.`, offset 1 is `..`, and listing entry `i` is offset `i + 2`. The offset sent with an entry is where the next call resumes. An entry that does not fit is not sent.
        if offset == 0 {
            if reply.add(ino, 1, FileType::Directory, ".") {
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
            let page = match self
                .vfs
                .read_folder(HandleId(fh.0), (offset - 2) as usize, PAGE)
            {
                Ok(page) => page,
                Err(error) => return reply.error(errno(&error)),
            };
            if page.is_empty() {
                return reply.ok();
            }
            for entry in page {
                offset += 1;
                if entry.name.len() > NAME_MAX {
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

    fn statfs(&self, _req: &Request, _ino: INodeNo, reply: ReplyStatfs) {
        let vfs = Arc::clone(&self.vfs);
        self.runtime.spawn(async move {
            let block = u64::from(BLOCK_SIZE);
            match vfs.space().await {
                Ok(space) => reply.statfs(
                    space.total / block,
                    space.free / block,
                    space.free / block,
                    0,
                    0,
                    BLOCK_SIZE,
                    NAME_MAX as u32,
                    BLOCK_SIZE,
                ),
                Err(error) => reply.error(errno(&error)),
            }
        });
    }
}

fn file_attr(node: NodeId, attributes: &Attributes, uid: u32, gid: u32) -> FileAttr {
    let (kind, perm, nlink) = match attributes.kind {
        Kind::Folder => (FileType::Directory, 0o500, 2),
        Kind::File => (FileType::RegularFile, 0o400, 1),
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
        uid,
        gid,
        rdev: 0,
        blksize: BLOCK_SIZE,
        flags: 0,
    }
}

// Never map a provider error to ENOSYS. The kernel takes it to mean this filesystem has no such call and stops sending that call for the life of the mount. Only the callbacks this adapter leaves out answer it, through `fuser`'s defaults.
fn errno(error: &VfsError) -> Errno {
    match error {
        VfsError::NotFound => Errno::ENOENT,
        VfsError::AccessDenied => Errno::EACCES,
        VfsError::ReadOnly => Errno::EROFS,
        VfsError::Conflict => Errno::EEXIST,
        VfsError::NotAFolder => Errno::ENOTDIR,
        VfsError::IsAFolder => Errno::EISDIR,
        VfsError::NoSpace => Errno::ENOSPC,
        VfsError::SignInNeeded => Errno::EACCES,
        VfsError::TimedOut => Errno::ETIMEDOUT,
        VfsError::Offline => Errno::ENETUNREACH,
        VfsError::NotSupported => Errno::EOPNOTSUPP,
        VfsError::StaleHandle => Errno::ESTALE,
        VfsError::Other(_) => Errno::EIO,
    }
}

#[cfg(test)]
mod tests {
    use std::time::UNIX_EPOCH;

    use super::*;

    fn attributes(kind: Kind, size: u64) -> Attributes {
        Attributes {
            kind,
            size,
            modified: UNIX_EPOCH + Duration::from_secs(9),
        }
    }

    #[test]
    fn file_attr_for_a_file() {
        let modified = UNIX_EPOCH + Duration::from_secs(9);
        let attr = file_attr(NodeId(7), &attributes(Kind::File, 1000), 11, 22);
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
    }

    #[test]
    fn file_attr_for_a_folder() {
        let modified = UNIX_EPOCH + Duration::from_secs(9);
        let attr = file_attr(NodeId(4), &attributes(Kind::Folder, 40), 11, 22);
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
    }
}
