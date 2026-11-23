use std::ffi::{CStr, CString};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use marlin_vfs::{Attributes, HandleId, IfExists, Kind, NodeId, Space, Vfs};

use crate::Shown;

use super::sys::{
    FuseBuf, FuseBufvec, FuseEntryParam, FuseFileInfo, FuseLowlevelOps, Req, Sys,
    FUSE_SET_ATTR_MTIME, FUSE_SET_ATTR_MTIME_NOW, FUSE_SET_ATTR_SIZE,
};
use super::Life;

/// The core keeps no parent for a node. libfuse uses this inode (`FUSE_UNKNOWN_INO`) when a filesystem does not know an entry's inode.
const UNKNOWN_INODE: u64 = 0xffff_ffff;

/// Serves one `Vfs`. A callback that may reach the provider answers from a tokio task, so a slow call never holds up the session thread. `forget`, `opendir`, `readdir` past offset 0, and the two releases answer in place. They touch the core's own state, and the last writer's release also writes the file's small spool record, with no sync. `flush` is left out: every write is in the spool before it answers, and macFUSE closes fine without it. `mknod` is left out because FSKit uses `create`. `fsyncdir` is left out. `access` is left out: macOS asks it for `W_OK` before it renames or removes a file too, so an answer from the mode would stop a rename or a trash of a read-only file. The xattr calls are left out: macFUSE then keeps extended attributes in `._name` files, which the core keeps on this computer, and answering `ENOTSUP` does not stop those files. `readlink` is left out: a mount shows no symbolic links.
pub(super) struct Fs {
    pub(super) vfs: Arc<Vfs>,
    pub(super) runtime: tokio::runtime::Handle,
    pub(super) shown: Shown,
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
        let param = entry_param(node, attr);
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

    fn write(mut self, count: usize) {
        // SAFETY: this request has not been answered. `count` is the number of bytes stored.
        unsafe { (self.sys.reply_write)(self.req, count) };
        self.answered = true;
    }

    fn create(mut self, node: NodeId, attr: libc::stat, fi: &FuseFileInfo) {
        let param = entry_param(node, attr);
        // SAFETY: this request has not been answered. `param` matches `fuse_entry_param` with Darwin extensions off, and `fi` is the file info the kernel sent, plus the handle id.
        unsafe { (self.sys.reply_create)(self.req, &param, fi) };
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

fn entry_param(node: NodeId, attr: libc::stat) -> FuseEntryParam {
    FuseEntryParam {
        ino: node.0,
        generation: 0,
        attr,
        attr_timeout: crate::TTL.as_secs_f64(),
        entry_timeout: crate::TTL.as_secs_f64(),
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
    setattr: Some(ll_setattr),
    readlink: None,
    mknod: None,
    mkdir: Some(ll_mkdir),
    unlink: Some(ll_unlink),
    rmdir: Some(ll_rmdir),
    symlink: Some(ll_symlink),
    rename: Some(ll_rename),
    link: Some(ll_link),
    open: Some(ll_open),
    read: Some(ll_read),
    write: Some(ll_write),
    flush: None,
    release: Some(ll_release),
    fsync: Some(ll_fsync),
    opendir: Some(ll_opendir),
    readdir: Some(ll_readdir),
    releasedir: Some(ll_release),
    fsyncdir: None,
    statfs: Some(ll_statfs),
    setxattr: None,
    getxattr: None,
    listxattr: None,
    removexattr: None,
    access: None,
    create: Some(ll_create),
};

extern "C" fn ll_lookup(req: Req, parent: u64, name: *const libc::c_char) {
    // SAFETY: libfuse calls this with a request for this session.
    let fs = unsafe { session(req) };
    let reply = Reply::new(fs, req);
    // SAFETY: `name` is a NUL-terminated string that lives for this callback.
    let name = unsafe { CStr::from_ptr(name) };
    let name = match crate::utf8_name(name.to_bytes(), crate::Name::Existing) {
        Ok(name) => name,
        Err(errno) => return reply.err(errno),
    };
    let vfs = Arc::clone(&fs.vfs);
    let shown = fs.shown;
    fs.runtime.spawn(async move {
        match vfs.lookup(NodeId(parent), &name).await {
            Ok(entry) => reply.entry(entry.node, shown.stat(entry.node, &entry.attributes)),
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
    let shown = fs.shown;
    fs.runtime.spawn(async move {
        let node = NodeId(ino);
        match vfs.attributes(node).await {
            Ok(attributes) => reply.attr(&shown.stat(node, &attributes)),
            Err(error) => reply.err(crate::errno(&error)),
        }
    });
}

extern "C" fn ll_setattr(
    req: Req,
    ino: u64,
    attr: *mut libc::stat,
    to_set: libc::c_int,
    _fi: *mut FuseFileInfo,
) {
    // SAFETY: libfuse calls this with a request for this session.
    let fs = unsafe { session(req) };
    let reply = Reply::new(fs, req);
    // libfuse leaves every `struct stat` member but the ones `to_set` names undefined.
    // An open with O_TRUNC arrives as a size of 0: macFUSE does not offer
    // FUSE_CAP_ATOMIC_O_TRUNC. The core cuts a clean file to zero without reading it.
    let size = (to_set & FUSE_SET_ATTR_SIZE != 0).then(|| {
        // SAFETY: `attr` lives for this callback, and the size bit says `st_size` is set.
        unsafe { (*attr).st_size as u64 }
    });
    let mtime = if to_set & FUSE_SET_ATTR_MTIME_NOW != 0 {
        Some(SystemTime::now())
    } else if to_set & FUSE_SET_ATTR_MTIME != 0 {
        // SAFETY: `attr` lives for this callback, and the mtime bit says these two members are set.
        let time = unsafe { from_timespec((*attr).st_mtime, (*attr).st_mtime_nsec) };
        match time {
            Some(time) => Some(time),
            None => return reply.err(libc::EINVAL),
        }
    } else {
        None
    };
    let vfs = Arc::clone(&fs.vfs);
    let shown = fs.shown;
    fs.runtime.spawn(async move {
        let node = NodeId(ino);
        match crate::set_attributes(&vfs, node, size, mtime).await {
            Ok(attributes) => reply.attr(&shown.stat(node, &attributes)),
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
    let access = crate::open_access(fi.flags);
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

extern "C" fn ll_write(
    req: Req,
    _ino: u64,
    buf: *const libc::c_char,
    size: usize,
    off: i64,
    fi: *mut FuseFileInfo,
) {
    // SAFETY: libfuse calls this with a request for this session.
    let fs = unsafe { session(req) };
    let reply = Reply::new(fs, req);
    // SAFETY: libfuse passes `size` readable bytes that live for this callback.
    let data = unsafe { std::slice::from_raw_parts(buf.cast::<u8>(), size) }.to_vec();
    // SAFETY: libfuse passes its own file info to this callback, and it is valid until the callback returns.
    let fh = unsafe { (*fi).fh };
    let vfs = Arc::clone(&fs.vfs);
    fs.runtime.spawn(async move {
        match vfs.write(HandleId(fh), off as u64, data).await {
            Ok(()) => reply.write(size),
            Err(error) => reply.err(crate::errno(&error)),
        }
    });
}

/// Serves `release` and `releasedir`: the core closes a file handle and a folder handle the same way.
extern "C" fn ll_release(req: Req, _ino: u64, fi: *mut FuseFileInfo) {
    // SAFETY: libfuse calls this with a request for this session.
    let fs = unsafe { session(req) };
    let reply = Reply::new(fs, req);
    // SAFETY: libfuse passes its own file info to this callback, and it is valid until the callback returns.
    let fh = unsafe { (*fi).fh };
    fs.vfs.release(HandleId(fh));
    reply.err(0);
}

/// macFUSE 5.5.0 never sends this: it writes an app's cached bytes to the adapter, then answers the app's `fsync` itself. Served for a macFUSE that does send it.
extern "C" fn ll_fsync(req: Req, ino: u64, _datasync: libc::c_int, _fi: *mut FuseFileInfo) {
    // SAFETY: libfuse calls this with a request for this session.
    let fs = unsafe { session(req) };
    let reply = Reply::new(fs, req);
    let vfs = Arc::clone(&fs.vfs);
    fs.runtime.spawn(async move {
        match vfs.fsync(NodeId(ino)).await {
            Ok(()) => reply.err(0),
            Err(error) => reply.err(crate::errno(&error)),
        }
    });
}

extern "C" fn ll_opendir(req: Req, ino: u64, fi: *mut FuseFileInfo) {
    // SAFETY: libfuse calls this with a request for this session.
    let fs = unsafe { session(req) };
    let reply = Reply::new(fs, req);
    // SAFETY: libfuse passes its own file info to this callback, and it is valid until the callback returns.
    let mut fi = unsafe { *fi };
    match fs.vfs.open_folder(NodeId(ino)) {
        Ok(handle) => {
            fi.fh = handle.0;
            reply.open(&fi);
        }
        Err(error) => reply.err(crate::errno(&error)),
    }
}

extern "C" fn ll_readdir(req: Req, ino: u64, size: usize, off: i64, fi: *mut FuseFileInfo) {
    // SAFETY: libfuse calls this with a request for this session.
    let fs = unsafe { session(req) };
    let reply = Reply::new(fs, req);
    // SAFETY: libfuse passes its own file info to this callback, and it is valid until the callback returns.
    let fh = unsafe { (*fi).fh };
    // The listing is taken at every read from offset 0. macFUSE keeps a folder handle open and re-reads it from offset 0 for every new listing.
    if off == 0 {
        let vfs = Arc::clone(&fs.vfs);
        fs.runtime.spawn(async move {
            if let Err(error) = vfs.rewind_folder(HandleId(fh)).await {
                reply.err(crate::errno(&error));
                return;
            }
            fill_dir(&vfs, reply, ino, fh, 0, size);
        });
        return;
    }
    fill_dir(&fs.vfs, reply, ino, fh, off, size);
}

/// Fills one readdir reply from the frozen listing. Offset 0 is `.`, offset 1 is `..`, and listing entry `i` is offset `i + 2`. The offset sent with an entry is where the next call resumes. An entry that does not fit is not sent.
fn fill_dir(vfs: &Vfs, reply: Reply, parent: u64, fh: u64, off: i64, size: usize) {
    let mut offset = off as u64;
    let mut buf = vec![0u8; size];
    let mut used = 0;
    if offset == 0 {
        if add(&reply, &mut buf, &mut used, parent, Kind::Folder, ".", 1) {
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
        let page = match vfs.read_folder(HandleId(fh), (offset - 2) as usize, crate::PAGE) {
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

extern "C" fn ll_mkdir(req: Req, parent: u64, name: *const libc::c_char, _mode: libc::mode_t) {
    // SAFETY: libfuse calls this with a request for this session.
    let fs = unsafe { session(req) };
    let reply = Reply::new(fs, req);
    // SAFETY: `name` is a NUL-terminated string that lives for this callback.
    let name = unsafe { CStr::from_ptr(name) };
    let name = match crate::utf8_name(name.to_bytes(), crate::Name::New) {
        Ok(name) => name,
        Err(errno) => return reply.err(errno),
    };
    let vfs = Arc::clone(&fs.vfs);
    let shown = fs.shown;
    fs.runtime.spawn(async move {
        match vfs.create_folder(NodeId(parent), &name).await {
            Ok(entry) => reply.entry(entry.node, shown.stat(entry.node, &entry.attributes)),
            Err(error) => reply.err(crate::errno(&error)),
        }
    });
}

extern "C" fn ll_unlink(req: Req, parent: u64, name: *const libc::c_char) {
    // SAFETY: libfuse calls this with a request for this session.
    let fs = unsafe { session(req) };
    let reply = Reply::new(fs, req);
    // SAFETY: `name` is a NUL-terminated string that lives for this callback.
    let name = unsafe { CStr::from_ptr(name) };
    let name = match crate::utf8_name(name.to_bytes(), crate::Name::Existing) {
        Ok(name) => name,
        Err(errno) => return reply.err(errno),
    };
    let vfs = Arc::clone(&fs.vfs);
    fs.runtime.spawn(async move {
        match vfs.remove(NodeId(parent), &name, Kind::File).await {
            Ok(()) => reply.err(0),
            Err(error) => reply.err(crate::errno(&error)),
        }
    });
}

extern "C" fn ll_rmdir(req: Req, parent: u64, name: *const libc::c_char) {
    // SAFETY: libfuse calls this with a request for this session.
    let fs = unsafe { session(req) };
    let reply = Reply::new(fs, req);
    // SAFETY: `name` is a NUL-terminated string that lives for this callback.
    let name = unsafe { CStr::from_ptr(name) };
    let name = match crate::utf8_name(name.to_bytes(), crate::Name::Existing) {
        Ok(name) => name,
        Err(errno) => return reply.err(errno),
    };
    let vfs = Arc::clone(&fs.vfs);
    fs.runtime.spawn(async move {
        match vfs.remove(NodeId(parent), &name, Kind::Folder).await {
            Ok(()) => reply.err(0),
            Err(error) => reply.err(crate::errno(&error)),
        }
    });
}

extern "C" fn ll_rename(
    req: Req,
    parent: u64,
    name: *const libc::c_char,
    newparent: u64,
    newname: *const libc::c_char,
    flags: libc::c_uint,
) {
    // SAFETY: libfuse calls this with a request for this session.
    let fs = unsafe { session(req) };
    let reply = Reply::new(fs, req);
    let if_exists = match if_exists(flags) {
        Ok(if_exists) => if_exists,
        Err(errno) => return reply.err(errno),
    };
    // A folder moved into itself never reaches here. macOS refuses it with EINVAL.
    // SAFETY: `name` is a NUL-terminated string that lives for this callback.
    let name = unsafe { CStr::from_ptr(name) };
    let name = match crate::utf8_name(name.to_bytes(), crate::Name::Existing) {
        Ok(name) => name,
        Err(errno) => return reply.err(errno),
    };
    // SAFETY: `newname` is a NUL-terminated string that lives for this callback.
    let newname = unsafe { CStr::from_ptr(newname) };
    let newname = match crate::utf8_name(newname.to_bytes(), crate::Name::New) {
        Ok(name) => name,
        Err(errno) => return reply.err(errno),
    };
    let vfs = Arc::clone(&fs.vfs);
    fs.runtime.spawn(async move {
        match vfs
            .rename(
                NodeId(parent),
                &name,
                NodeId(newparent),
                &newname,
                if_exists,
            )
            .await
        {
            Ok(()) => reply.err(0),
            Err(error) => reply.err(crate::errno(&error)),
        }
    });
}

extern "C" fn ll_create(
    req: Req,
    parent: u64,
    name: *const libc::c_char,
    _mode: libc::mode_t,
    fi: *mut FuseFileInfo,
) {
    // SAFETY: libfuse calls this with a request for this session.
    let fs = unsafe { session(req) };
    let reply = Reply::new(fs, req);
    // SAFETY: libfuse passes its own file info to this callback, and it is valid until the callback returns.
    let mut fi = unsafe { *fi };
    // SAFETY: `name` is a NUL-terminated string that lives for this callback.
    let name = unsafe { CStr::from_ptr(name) };
    let name = match crate::utf8_name(name.to_bytes(), crate::Name::New) {
        Ok(name) => name,
        Err(errno) => return reply.err(errno),
    };
    let access = crate::open_access(fi.flags);
    let vfs = Arc::clone(&fs.vfs);
    let shown = fs.shown;
    fs.runtime.spawn(async move {
        match vfs.create(NodeId(parent), &name, access).await {
            Ok((entry, handle)) => {
                fi.fh = handle.0;
                reply.create(entry.node, shown.stat(entry.node, &entry.attributes), &fi);
            }
            Err(error) => reply.err(crate::errno(&error)),
        }
    });
}

/// A mount has no hard links (decision 0005). `ENOTSUP` is what `link(2)` on macOS answers for a filesystem without them. Left out, the app would get libfuse's `ENOSYS`, "Function not implemented".
extern "C" fn ll_link(req: Req, _ino: u64, _newparent: u64, _newname: *const libc::c_char) {
    // SAFETY: libfuse calls this with a request for this session.
    let fs = unsafe { session(req) };
    Reply::new(fs, req).err(libc::ENOTSUP);
}

/// A mount makes no new symbolic links (decision 0005), and answers like `ll_link`.
extern "C" fn ll_symlink(
    req: Req,
    _target: *const libc::c_char,
    _parent: u64,
    _name: *const libc::c_char,
) {
    // SAFETY: libfuse calls this with a request for this session.
    let fs = unsafe { session(req) };
    Reply::new(fs, req).err(libc::ENOTSUP);
}

extern "C" fn ll_statfs(req: Req, _ino: u64) {
    // SAFETY: libfuse calls this with a request for this session.
    let fs = unsafe { session(req) };
    let reply = Reply::new(fs, req);
    let vfs = Arc::clone(&fs.vfs);
    fs.runtime.spawn(async move {
        match vfs.space().await {
            Ok(space) => reply.statfs(&fill_statvfs(&space)),
            Err(error) => reply.err(crate::errno(&error)),
        }
    });
}

fn fill_statvfs(space: &Space) -> libc::statvfs {
    let block = u64::from(crate::BLOCK_SIZE);
    // macOS counts blocks in 32 bits. A bigger account reports the most that fits, which is 16 TiB.
    let blocks = |bytes: u64| u32::try_from(bytes / block).unwrap_or(u32::MAX);
    // SAFETY: statvfs is plain numbers. Zero leaves the counts this filesystem does not report at zero.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    st.f_bsize = block;
    st.f_frsize = block;
    st.f_blocks = blocks(space.total);
    st.f_bfree = blocks(space.free);
    st.f_bavail = blocks(space.free);
    st.f_namemax = crate::NAME_MAX as _;
    st
}

/// What a rename does when the new name is taken. macFUSE sends `RENAME_EXCL` for a rename onto a free name, and no flag for a rename over an item. `RENAME_SWAP` is `ENOTSUP`, which is what macOS answers for a rename flag a filesystem does not support.
fn if_exists(flags: libc::c_uint) -> Result<IfExists, libc::c_int> {
    if flags & libc::RENAME_SWAP != 0 {
        return Err(libc::ENOTSUP);
    }
    if flags & libc::RENAME_EXCL != 0 {
        Ok(IfExists::Refuse)
    } else {
        Ok(IfExists::Replace)
    }
}

/// `time` as seconds and nanoseconds. `nsec` is in `0..1_000_000_000`, including for a time before 1970.
fn to_timespec(time: SystemTime) -> (libc::time_t, libc::c_long) {
    match time.duration_since(UNIX_EPOCH) {
        Ok(after) => (
            after.as_secs() as libc::time_t,
            libc::c_long::from(after.subsec_nanos()),
        ),
        Err(early) => {
            let before = early.duration();
            let mut sec = -(before.as_secs() as i64);
            let nsec = if before.subsec_nanos() == 0 {
                0
            } else {
                sec -= 1;
                1_000_000_000 - before.subsec_nanos()
            };
            (sec as libc::time_t, libc::c_long::from(nsec))
        }
    }
}

/// The instant as a `SystemTime`, or `None` when the numbers cannot become one. A panic here would abort the helper and every mount.
fn from_timespec(sec: libc::time_t, nsec: libc::c_long) -> Option<SystemTime> {
    if sec >= 0 {
        UNIX_EPOCH.checked_add(Duration::new(sec as u64, nsec as u32))
    } else if nsec == 0 {
        // `checked_neg` refuses `time_t::MIN`, whose negation would panic.
        UNIX_EPOCH.checked_sub(Duration::new(sec.checked_neg()? as u64, 0))
    } else {
        UNIX_EPOCH.checked_sub(Duration::new(
            (sec.checked_neg()? as u64) - 1,
            (1_000_000_000 - nsec) as u32,
        ))
    }
}

impl Shown {
    fn stat(self, node: NodeId, attributes: &Attributes) -> libc::stat {
        // SAFETY: stat is plain numbers. Zero is a valid value for the fields this filesystem does not use.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        let (perm, nlink) = self.mode_and_links(attributes);
        let (sec, nsec) = to_timespec(attributes.modified);
        st.st_ino = node.0;
        st.st_mode = match attributes.kind {
            Kind::Folder => libc::S_IFDIR,
            Kind::File => libc::S_IFREG,
        } | perm;
        st.st_nlink = nlink as libc::nlink_t;
        st.st_uid = self.uid;
        st.st_gid = self.gid;
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
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use marlin_vfs::{Attributes, Kind, NodeId};

    use super::*;

    use crate::Shown;

    fn attributes(kind: Kind, size: u64) -> Attributes {
        Attributes {
            kind,
            size,
            modified: UNIX_EPOCH + Duration::from_secs(9),
            revision: None,
            read_only: false,
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
    fn stat_for_a_file() {
        let st = shown(false).stat(NodeId(7), &attributes(Kind::File, 1000));
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
        assert_eq!(
            shown(true)
                .stat(NodeId(7), &attributes(Kind::File, 1000))
                .st_mode
                & 0o777,
            0o600
        );
        let mut read_only = attributes(Kind::File, 1000);
        read_only.read_only = true;
        assert_eq!(
            shown(true).stat(NodeId(7), &read_only).st_mode & 0o777,
            0o400
        );
    }

    #[test]
    fn free_space_counts_blocks_and_stops_at_what_32_bits_hold() {
        let small = fill_statvfs(&Space {
            total: 1_000_000,
            free: 250_000,
        });
        assert_eq!(small.f_frsize, 4096);
        assert_eq!(small.f_blocks, 244);
        assert_eq!(small.f_bfree, 61);
        assert_eq!(small.f_bavail, 61);
        assert_eq!(small.f_namemax, 255);

        // What the Google provider reports for an account with no limit.
        let no_limit = fill_statvfs(&Space {
            total: 5_000 + (1 << 50),
            free: 1 << 50,
        });
        assert_eq!(no_limit.f_blocks, u32::MAX);
        assert_eq!(no_limit.f_bfree, u32::MAX);
        assert_eq!(no_limit.f_bavail, u32::MAX);
    }

    #[test]
    fn stat_for_a_folder() {
        let st = shown(false).stat(NodeId(4), &attributes(Kind::Folder, 40));
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
        assert_eq!(
            shown(true)
                .stat(NodeId(4), &attributes(Kind::Folder, 40))
                .st_mode
                & 0o777,
            0o700
        );
    }

    #[test]
    fn rename_flags_map_to_what_a_taken_name_does() {
        assert_eq!(if_exists(0), Ok(IfExists::Replace));
        assert_eq!(if_exists(libc::RENAME_EXCL), Ok(IfExists::Refuse));
        assert_eq!(if_exists(libc::RENAME_SWAP), Err(libc::ENOTSUP));
    }

    #[test]
    fn a_time_round_trips_through_a_timespec() {
        for time in [
            UNIX_EPOCH - Duration::new(5, 250_000_000),
            UNIX_EPOCH,
            UNIX_EPOCH + Duration::new(9, 1),
        ] {
            let (sec, nsec) = to_timespec(time);
            assert_eq!(from_timespec(sec, nsec), Some(time));
        }
        assert_eq!(from_timespec(libc::time_t::MIN, 0), None);
        assert_eq!(from_timespec(libc::time_t::MIN, 1), None);
    }
}
