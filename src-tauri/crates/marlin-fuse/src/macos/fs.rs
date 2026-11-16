use std::ffi::{CStr, CString};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use marlin_vfs::{Access, Attributes, HandleId, Kind, NodeId, Vfs};

use super::sys::{FuseBuf, FuseBufvec, FuseEntryParam, FuseFileInfo, FuseLowlevelOps, Req, Sys};
use super::Life;

/// The core keeps no parent for a node. libfuse uses this inode (`FUSE_UNKNOWN_INO`) when a filesystem does not know an entry's inode.
const UNKNOWN_INODE: u64 = 0xffff_ffff;

/// Serves one `Vfs`. A callback that may reach the provider answers from a tokio task, so a slow call never holds up the session thread. `forget`, `readdir`, and the two releases only touch the core's own state and answer in place.
pub(super) struct Fs {
    pub(super) vfs: Arc<Vfs>,
    pub(super) runtime: tokio::runtime::Handle,
    pub(super) uid: u32,
    pub(super) gid: u32,
    pub(super) sys: &'static Sys,
    pub(super) life: Arc<Life>,
}

/// One request that has not been answered. Dropping it without a reply answers `EIO`, so a cancelled task still answers exactly once.
struct Reply {
    req: Req,
    sys: &'static Sys,
    life: Arc<Life>,
    answered: bool,
}

// SAFETY: the request is answered from one task. libfuse accepts that reply on a thread other than the session loop.
unsafe impl Send for Reply {}

impl Reply {
    fn new(fs: &Fs, req: Req) -> Self {
        fs.life.begin_request();
        Self {
            req,
            sys: fs.sys,
            life: Arc::clone(&fs.life),
            answered: false,
        }
    }

    fn err(mut self, err: libc::c_int) {
        // SAFETY: this request has not been answered, and `err` is a positive errno or zero.
        unsafe { (self.sys.reply_err)(self.req, err) };
        self.answered = true;
    }

    fn none(mut self) {
        // SAFETY: `forget` is answered with no payload, once.
        unsafe { (self.sys.reply_none)(self.req) };
        self.answered = true;
    }

    fn entry(mut self, node: NodeId, attr: libc::stat) {
        let param = FuseEntryParam {
            ino: node.0,
            generation: 0,
            attr,
            attr_timeout: crate::TTL.as_secs_f64(),
            entry_timeout: crate::TTL.as_secs_f64(),
        };
        // SAFETY: this request has not been answered. `param` matches `fuse_entry_param` with Darwin extensions off.
        unsafe { (self.sys.reply_entry)(self.req, &param) };
        self.answered = true;
    }

    fn attr(mut self, attr: &libc::stat) {
        // SAFETY: this request has not been answered. `attr` is the system `struct stat`.
        unsafe { (self.sys.reply_attr)(self.req, attr, crate::TTL.as_secs_f64()) };
        self.answered = true;
    }

    fn open(mut self, fi: &FuseFileInfo) {
        // SAFETY: this request has not been answered. `fi` is the file info the kernel sent, plus the handle id.
        unsafe { (self.sys.reply_open)(self.req, fi) };
        self.answered = true;
    }

    fn buf(mut self, bytes: &[u8]) {
        let mut data = FuseBufvec {
            count: 1,
            idx: 0,
            off: 0,
            buf: [FuseBuf {
                size: bytes.len(),
                flags: 0,
                mem: bytes.as_ptr().cast_mut().cast(),
                fd: -1,
                pos: 0,
                mem_size: 0,
            }],
        };
        // Not `fuse_reply_buf`. With the FSKit backend of macFUSE 5.5.0, that call makes a read of 64 KiB or more fail in the app with EIO.
        // SAFETY: this request has not been answered. libfuse copies `bytes` before it returns, and it does not write to them.
        unsafe { (self.sys.reply_data)(self.req, &mut data, 0) };
        self.answered = true;
    }

    fn statfs(mut self, st: &libc::statvfs) {
        // SAFETY: this request has not been answered. `st` is the system `struct statvfs`.
        unsafe { (self.sys.reply_statfs)(self.req, st) };
        self.answered = true;
    }
}

impl Drop for Reply {
    fn drop(&mut self) {
        if !self.answered {
            // SAFETY: the task was dropped before it answered. One `EIO` reply frees the request.
            unsafe { (self.sys.reply_err)(self.req, libc::EIO) };
        }
        self.life.end_request();
    }
}

/// The `Fs` for this request.
///
/// # Safety
/// `req` must be a live request whose userdata is the `Fs` for its session. That `Fs` outlives the callback: the session thread frees it only after every request is answered.
unsafe fn session(req: Req) -> &'static Fs {
    let sys = super::sys::LOADED
        .get()
        .expect("libfuse calls back only after it loaded");
    // SAFETY: the caller guarantees `req`'s userdata is our `Fs`.
    unsafe { &*((sys.req_userdata)(req) as *const Fs) }
}

pub(super) static OPS: FuseLowlevelOps = FuseLowlevelOps {
    init: None,
    destroy: None,
    lookup: Some(ll_lookup),
    forget: Some(ll_forget),
    getattr: Some(ll_getattr),
    setattr: None,
    readlink: None,
    mknod: None,
    mkdir: None,
    unlink: None,
    rmdir: None,
    symlink: None,
    rename: None,
    link: None,
    open: Some(ll_open),
    read: Some(ll_read),
    write: None,
    flush: None,
    release: Some(ll_release),
    fsync: None,
    opendir: Some(ll_opendir),
    readdir: Some(ll_readdir),
    releasedir: Some(ll_releasedir),
    fsyncdir: None,
    statfs: Some(ll_statfs),
};

extern "C" fn ll_lookup(req: Req, parent: u64, name: *const libc::c_char) {
    // SAFETY: libfuse calls this with a request for this session.
    let fs = unsafe { session(req) };
    let reply = Reply::new(fs, req);
    // SAFETY: `name` is a NUL-terminated string that lives for this callback.
    let name = unsafe { CStr::from_ptr(name) };
    let Ok(name) = name.to_str() else {
        reply.err(libc::ENOENT);
        return;
    };
    if name.len() > crate::NAME_MAX {
        reply.err(libc::ENAMETOOLONG);
        return;
    }
    let name = name.to_owned();
    let vfs = Arc::clone(&fs.vfs);
    let uid = fs.uid;
    let gid = fs.gid;
    fs.runtime.spawn(async move {
        match vfs.lookup(NodeId(parent), &name).await {
            Ok(entry) => {
                let attr = fill_stat(entry.node, &entry.attributes, uid, gid);
                reply.entry(entry.node, attr);
            }
            Err(error) => reply.err(crate::errno(&error)),
        }
    });
}

extern "C" fn ll_forget(req: Req, ino: u64, nlookup: u64) {
    // SAFETY: libfuse calls this with a request for this session.
    let fs = unsafe { session(req) };
    let reply = Reply::new(fs, req);
    fs.vfs.forget(NodeId(ino), nlookup);
    reply.none();
}

extern "C" fn ll_getattr(req: Req, ino: u64, _fi: *mut FuseFileInfo) {
    // SAFETY: libfuse calls this with a request for this session.
    let fs = unsafe { session(req) };
    let reply = Reply::new(fs, req);
    let vfs = Arc::clone(&fs.vfs);
    let uid = fs.uid;
    let gid = fs.gid;
    fs.runtime.spawn(async move {
        let node = NodeId(ino);
        match vfs.attributes(node).await {
            Ok(attributes) => {
                let attr = fill_stat(node, &attributes, uid, gid);
                reply.attr(&attr);
            }
            Err(error) => reply.err(crate::errno(&error)),
        }
    });
}

extern "C" fn ll_open(req: Req, ino: u64, fi: *mut FuseFileInfo) {
    // SAFETY: libfuse calls this with a request for this session.
    let fs = unsafe { session(req) };
    let reply = Reply::new(fs, req);
    // SAFETY: libfuse passes its own file info to this callback, and it is valid until the callback returns.
    let mut fi = unsafe { *fi };
    let access = if fi.flags & libc::O_ACCMODE == libc::O_RDONLY {
        Access::Read
    } else {
        Access::Write
    };
    let vfs = Arc::clone(&fs.vfs);
    fs.runtime.spawn(async move {
        match vfs.open(NodeId(ino), access).await {
            Ok(handle) => {
                // Leave `keep_cache` clear. macOS is not asked to drop cached bytes when the file changes, so a kept page would go stale.
                fi.fh = handle.0;
                reply.open(&fi);
            }
            Err(error) => reply.err(crate::errno(&error)),
        }
    });
}

extern "C" fn ll_read(req: Req, _ino: u64, size: usize, off: i64, fi: *mut FuseFileInfo) {
    // SAFETY: libfuse calls this with a request for this session.
    let fs = unsafe { session(req) };
    let reply = Reply::new(fs, req);
    // SAFETY: libfuse passes its own file info to this callback, and it is valid until the callback returns.
    let fh = unsafe { (*fi).fh };
    let vfs = Arc::clone(&fs.vfs);
    let length = u32::try_from(size).unwrap_or(u32::MAX);
    fs.runtime.spawn(async move {
        match vfs.read(HandleId(fh), off as u64, length).await {
            Ok(bytes) => reply.buf(&bytes),
            Err(error) => reply.err(crate::errno(&error)),
        }
    });
}

extern "C" fn ll_release(req: Req, _ino: u64, fi: *mut FuseFileInfo) {
    // SAFETY: libfuse calls this with a request for this session.
    let fs = unsafe { session(req) };
    let reply = Reply::new(fs, req);
    // SAFETY: libfuse passes its own file info to this callback, and it is valid until the callback returns.
    let fh = unsafe { (*fi).fh };
    fs.vfs.release(HandleId(fh));
    reply.err(0);
}

extern "C" fn ll_opendir(req: Req, ino: u64, fi: *mut FuseFileInfo) {
    // SAFETY: libfuse calls this with a request for this session.
    let fs = unsafe { session(req) };
    let reply = Reply::new(fs, req);
    // SAFETY: libfuse passes its own file info to this callback, and it is valid until the callback returns.
    let mut fi = unsafe { *fi };
    let vfs = Arc::clone(&fs.vfs);
    fs.runtime.spawn(async move {
        match vfs.open_folder(NodeId(ino)).await {
            Ok(handle) => {
                fi.fh = handle.0;
                reply.open(&fi);
            }
            Err(error) => reply.err(crate::errno(&error)),
        }
    });
}

extern "C" fn ll_readdir(req: Req, ino: u64, size: usize, off: i64, fi: *mut FuseFileInfo) {
    // SAFETY: libfuse calls this with a request for this session.
    let fs = unsafe { session(req) };
    let reply = Reply::new(fs, req);
    // SAFETY: libfuse passes its own file info to this callback, and it is valid until the callback returns.
    let fh = unsafe { (*fi).fh };
    // Offset 0 is `.`, offset 1 is `..`, and listing entry `i` is offset `i + 2`. The offset sent with an entry is where the next call resumes. An entry that does not fit is not sent.
    let mut offset = off as u64;
    let mut buf = vec![0u8; size];
    let mut used = 0;
    if offset == 0 {
        if add(&reply, &mut buf, &mut used, ino, Kind::Folder, ".", 1) {
            return reply.buf(&buf[..used]);
        }
        offset = 1;
    }
    if offset == 1 {
        if add(
            &reply,
            &mut buf,
            &mut used,
            UNKNOWN_INODE,
            Kind::Folder,
            "..",
            2,
        ) {
            return reply.buf(&buf[..used]);
        }
        offset = 2;
    }
    loop {
        let page = match fs
            .vfs
            .read_folder(HandleId(fh), (offset - 2) as usize, crate::PAGE)
        {
            Ok(page) => page,
            Err(error) => return reply.err(crate::errno(&error)),
        };
        if page.is_empty() {
            return reply.buf(&buf[..used]);
        }
        for entry in page {
            offset += 1;
            if entry.name.len() > crate::NAME_MAX {
                continue;
            }
            if add(
                &reply,
                &mut buf,
                &mut used,
                entry.node.0,
                entry.attributes.kind,
                &entry.name,
                offset as i64,
            ) {
                return reply.buf(&buf[..used]);
            }
        }
    }
}

/// Adds one entry to a readdir buffer. `next` is the offset where a later call resumes. Returns true when the entry does not fit: it was not written.
fn add(
    reply: &Reply,
    buf: &mut [u8],
    used: &mut usize,
    ino: u64,
    kind: Kind,
    name: &str,
    next: i64,
) -> bool {
    // A name with a NUL byte cannot be sent, so the listing moves on.
    let Ok(name) = CString::new(name) else {
        return false;
    };
    // SAFETY: stat is plain numbers. Zero is a valid value, and readdir only reads the inode and the type bits.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    st.st_ino = ino;
    st.st_mode = match kind {
        Kind::Folder => libc::S_IFDIR,
        Kind::File => libc::S_IFREG,
    };
    let free = &mut buf[*used..];
    // SAFETY: `free` is the unused tail of this reply's buffer, and `name` is NUL-terminated. libfuse writes the entry only when it fits, and returns the bytes it needs either way.
    let need = unsafe {
        (reply.sys.add_direntry)(
            reply.req,
            free.as_mut_ptr().cast(),
            free.len(),
            name.as_ptr(),
            &st,
            next,
        )
    };
    if need > free.len() {
        return true;
    }
    *used += need;
    false
}

extern "C" fn ll_releasedir(req: Req, _ino: u64, fi: *mut FuseFileInfo) {
    // SAFETY: libfuse calls this with a request for this session.
    let fs = unsafe { session(req) };
    let reply = Reply::new(fs, req);
    // SAFETY: libfuse passes its own file info to this callback, and it is valid until the callback returns.
    let fh = unsafe { (*fi).fh };
    fs.vfs.release(HandleId(fh));
    reply.err(0);
}

extern "C" fn ll_statfs(req: Req, _ino: u64) {
    // SAFETY: libfuse calls this with a request for this session.
    let fs = unsafe { session(req) };
    let reply = Reply::new(fs, req);
    let vfs = Arc::clone(&fs.vfs);
    fs.runtime.spawn(async move {
        let block = u64::from(crate::BLOCK_SIZE);
        match vfs.space().await {
            Ok(space) => {
                // SAFETY: statvfs is plain numbers. Zero leaves the counts we do not report at zero.
                let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
                st.f_bsize = u64::from(crate::BLOCK_SIZE);
                st.f_frsize = u64::from(crate::BLOCK_SIZE);
                st.f_blocks = (space.total / block) as _;
                st.f_bfree = (space.free / block) as _;
                st.f_bavail = (space.free / block) as _;
                st.f_namemax = crate::NAME_MAX as _;
                reply.statfs(&st);
            }
            Err(error) => reply.err(crate::errno(&error)),
        }
    });
}

fn fill_stat(node: NodeId, attributes: &Attributes, uid: u32, gid: u32) -> libc::stat {
    // SAFETY: stat is plain numbers. Zero is a valid value for the fields this filesystem does not use.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    let (perm, nlink) = crate::mode_and_links(attributes.kind);
    let since = attributes
        .modified
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let sec = since.as_secs() as libc::time_t;
    let nsec = libc::c_long::from(since.subsec_nanos());
    st.st_ino = node.0;
    st.st_mode = match attributes.kind {
        Kind::Folder => libc::S_IFDIR,
        Kind::File => libc::S_IFREG,
    } | perm;
    st.st_nlink = nlink as libc::nlink_t;
    st.st_uid = uid;
    st.st_gid = gid;
    st.st_size = attributes.size as libc::off_t;
    st.st_blocks = attributes.size.div_ceil(512) as libc::blkcnt_t;
    st.st_blksize = crate::BLOCK_SIZE as libc::blksize_t;
    st.st_atime = sec;
    st.st_atime_nsec = nsec;
    st.st_mtime = sec;
    st.st_mtime_nsec = nsec;
    st.st_ctime = sec;
    st.st_ctime_nsec = nsec;
    st.st_birthtime = sec;
    st.st_birthtime_nsec = nsec;
    st
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use marlin_vfs::{Attributes, Kind, NodeId};

    use super::*;

    fn attributes(kind: Kind, size: u64) -> Attributes {
        Attributes {
            kind,
            size,
            modified: UNIX_EPOCH + Duration::from_secs(9),
        }
    }

    #[test]
    fn fill_stat_for_a_file() {
        let st = fill_stat(NodeId(7), &attributes(Kind::File, 1000), 11, 22);
        assert_eq!(st.st_ino, 7);
        assert_eq!(st.st_mode & libc::S_IFMT, libc::S_IFREG);
        assert_eq!(st.st_mode & 0o777, 0o400);
        assert_eq!(st.st_nlink, 1);
        assert_eq!(st.st_size, 1000);
        assert_eq!(st.st_blocks, 2);
        assert_eq!(st.st_blksize, crate::BLOCK_SIZE as libc::blksize_t);
        assert_eq!(st.st_atime, 9);
        assert_eq!(st.st_mtime, 9);
        assert_eq!(st.st_ctime, 9);
        assert_eq!(st.st_birthtime, 9);
        assert_eq!(st.st_atime_nsec, 0);
        assert_eq!(st.st_uid, 11);
        assert_eq!(st.st_gid, 22);
    }

    #[test]
    fn fill_stat_for_a_folder() {
        let st = fill_stat(NodeId(4), &attributes(Kind::Folder, 40), 11, 22);
        assert_eq!(st.st_ino, 4);
        assert_eq!(st.st_mode & libc::S_IFMT, libc::S_IFDIR);
        assert_eq!(st.st_mode & 0o777, 0o500);
        assert_eq!(st.st_nlink, 2);
        assert_eq!(st.st_size, 40);
        assert_eq!(st.st_blocks, 1);
        assert_eq!(st.st_blksize, crate::BLOCK_SIZE as libc::blksize_t);
        assert_eq!(st.st_atime, 9);
        assert_eq!(st.st_mtime, 9);
        assert_eq!(st.st_ctime, 9);
        assert_eq!(st.st_birthtime, 9);
        assert_eq!(st.st_birthtime_nsec, 0);
        assert_eq!(st.st_uid, 11);
        assert_eq!(st.st_gid, 22);
    }
}
