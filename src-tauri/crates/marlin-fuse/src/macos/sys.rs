//! Bindings for the low-level API of macFUSE's libfuse 3.
//!
//! The structs match the headers with `FUSE_DARWIN_ENABLE_EXTENSIONS` off and `FUSE_USE_VERSION` 318. The library is loaded at run time, so a build does not need macFUSE installed.

use std::ffi::c_void;
use std::path::Path;
use std::sync::OnceLock;

use libloading::Library;

/// One libfuse request. A raw pointer, answered through the loaded functions.
pub(super) type Req = *mut c_void;

/// An opaque `fuse_session`.
pub(super) type Session = *mut c_void;

#[repr(C)]
pub(super) struct FuseArgs {
    pub argc: i32,
    pub argv: *mut *mut libc::c_char,
    pub allocated: i32,
}

#[repr(C)]
pub(super) struct LibfuseVersion {
    pub major: u32,
    pub minor: u32,
    pub hotfix: u32,
    /// Bit 0 is `darwin_extensions_enabled`. The other bits are padding.
    pub flags: u32,
}

/// `struct fuse_file_info`. The C bit fields are one word, low bit first:
/// 0 writepage, 1 direct_io, 2 keep_cache, 3 flush, 4 nonseekable,
/// 5 flock_release, 6 cache_readdir, 7 noflush, 8 parallel_direct_writes,
/// 9 kill_suidgid, and bits 10..31 are padding.
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct FuseFileInfo {
    pub flags: i32,
    pub bits: u32,
    pub padding2: u32,
    pub padding3: u32,
    pub fh: u64,
    pub lock_owner: u64,
    pub poll_events: u32,
    pub backing_id: i32,
    pub compat_flags: u64,
    pub reserved: [u64; 2],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct FuseEntryParam {
    pub ino: u64,
    pub generation: u64,
    pub attr: libc::stat,
    pub attr_timeout: f64,
    pub entry_timeout: f64,
}

/// `struct fuse_buf`: one piece of a reply.
#[repr(C)]
pub(super) struct FuseBuf {
    pub size: usize,
    pub flags: i32,
    pub mem: *mut c_void,
    pub fd: i32,
    pub pos: i64,
    pub mem_size: usize,
}

/// `struct fuse_bufvec` with its one buffer.
#[repr(C)]
pub(super) struct FuseBufvec {
    pub count: usize,
    pub idx: usize,
    pub off: usize,
    pub buf: [FuseBuf; 1],
}

/// `struct fuse_lowlevel_ops` through `create`. libfuse accepts a shorter struct when `op_size` says how much was passed.
#[repr(C)]
pub(super) struct FuseLowlevelOps {
    pub init: Option<extern "C" fn()>,
    pub destroy: Option<extern "C" fn()>,
    pub lookup: Option<extern "C" fn(Req, u64, *const libc::c_char)>,
    pub forget: Option<extern "C" fn(Req, u64, u64)>,
    pub getattr: Option<extern "C" fn(Req, u64, *mut FuseFileInfo)>,
    pub setattr: Option<extern "C" fn(Req, u64, *mut libc::stat, libc::c_int, *mut FuseFileInfo)>,
    pub readlink: Option<extern "C" fn()>,
    pub mknod: Option<extern "C" fn()>,
    pub mkdir: Option<extern "C" fn(Req, u64, *const libc::c_char, libc::mode_t)>,
    pub unlink: Option<extern "C" fn(Req, u64, *const libc::c_char)>,
    pub rmdir: Option<extern "C" fn(Req, u64, *const libc::c_char)>,
    pub symlink: Option<extern "C" fn(Req, *const libc::c_char, u64, *const libc::c_char)>,
    pub rename: Option<
        extern "C" fn(Req, u64, *const libc::c_char, u64, *const libc::c_char, libc::c_uint),
    >,
    pub link: Option<extern "C" fn(Req, u64, u64, *const libc::c_char)>,
    pub open: Option<extern "C" fn(Req, u64, *mut FuseFileInfo)>,
    pub read: Option<extern "C" fn(Req, u64, usize, i64, *mut FuseFileInfo)>,
    pub write: Option<extern "C" fn(Req, u64, *const libc::c_char, usize, i64, *mut FuseFileInfo)>,
    pub flush: Option<extern "C" fn()>,
    pub release: Option<extern "C" fn(Req, u64, *mut FuseFileInfo)>,
    pub fsync: Option<extern "C" fn(Req, u64, libc::c_int, *mut FuseFileInfo)>,
    pub opendir: Option<extern "C" fn(Req, u64, *mut FuseFileInfo)>,
    pub readdir: Option<extern "C" fn(Req, u64, usize, i64, *mut FuseFileInfo)>,
    pub releasedir: Option<extern "C" fn(Req, u64, *mut FuseFileInfo)>,
    pub fsyncdir: Option<extern "C" fn()>,
    pub statfs: Option<extern "C" fn(Req, u64)>,
    pub setxattr: Option<extern "C" fn()>,
    pub getxattr: Option<extern "C" fn()>,
    pub listxattr: Option<extern "C" fn()>,
    pub removexattr: Option<extern "C" fn()>,
    pub access: Option<extern "C" fn()>,
    pub create:
        Option<extern "C" fn(Req, u64, *const libc::c_char, libc::mode_t, *mut FuseFileInfo)>,
}

// `fuse_lowlevel.h`. Darwin extensions off does not change these values.
pub(super) const FUSE_SET_ATTR_SIZE: libc::c_int = 1 << 3;
pub(super) const FUSE_SET_ATTR_MTIME: libc::c_int = 1 << 5;
pub(super) const FUSE_SET_ATTR_MTIME_NOW: libc::c_int = 1 << 8;

type SessionNew = unsafe extern "C" fn(
    *mut FuseArgs,
    *const FuseLowlevelOps,
    usize,
    *mut LibfuseVersion,
    *mut c_void,
) -> Session;
type SessionMount = unsafe extern "C" fn(Session, *const libc::c_char) -> libc::c_int;
type SessionLoop = unsafe extern "C" fn(Session) -> libc::c_int;
type SessionUnmount = unsafe extern "C" fn(Session);
type SessionDestroy = unsafe extern "C" fn(Session);
type OptFreeArgs = unsafe extern "C" fn(*mut FuseArgs);
type ReqUserdata = unsafe extern "C" fn(Req) -> *mut c_void;
type ReplyErr = unsafe extern "C" fn(Req, libc::c_int) -> libc::c_int;
type ReplyNone = unsafe extern "C" fn(Req);
type ReplyEntry = unsafe extern "C" fn(Req, *const FuseEntryParam) -> libc::c_int;
type ReplyAttr = unsafe extern "C" fn(Req, *const libc::stat, f64) -> libc::c_int;
type ReplyOpen = unsafe extern "C" fn(Req, *const FuseFileInfo) -> libc::c_int;
type ReplyData = unsafe extern "C" fn(Req, *mut FuseBufvec, libc::c_int) -> libc::c_int;
type ReplyStatfs = unsafe extern "C" fn(Req, *const libc::statvfs) -> libc::c_int;
type ReplyWrite = unsafe extern "C" fn(Req, usize) -> libc::c_int;
type ReplyCreate =
    unsafe extern "C" fn(Req, *const FuseEntryParam, *const FuseFileInfo) -> libc::c_int;
type AddDirentry = unsafe extern "C" fn(
    Req,
    *mut libc::c_char,
    usize,
    *const libc::c_char,
    *const libc::stat,
    i64,
) -> usize;

pub(super) struct Sys {
    _lib: Library,
    pub session_new: SessionNew,
    pub session_mount: SessionMount,
    pub session_loop: SessionLoop,
    pub session_unmount: SessionUnmount,
    pub session_destroy: SessionDestroy,
    pub opt_free_args: OptFreeArgs,
    pub req_userdata: ReqUserdata,
    pub reply_err: ReplyErr,
    pub reply_none: ReplyNone,
    pub reply_entry: ReplyEntry,
    pub reply_attr: ReplyAttr,
    pub reply_open: ReplyOpen,
    pub reply_data: ReplyData,
    pub reply_statfs: ReplyStatfs,
    pub reply_write: ReplyWrite,
    pub reply_create: ReplyCreate,
    pub add_direntry: AddDirentry,
}

fn symbol<T: Copy>(lib: &Library, name: &[u8]) -> Result<T, ()> {
    // SAFETY: `name` is a NUL-terminated symbol that this process calls with the header's signature. The copy outlives only as long as `lib` stays loaded.
    unsafe { lib.get(name) }.map(|found| *found).map_err(|_| ())
}

/// The loaded library. libfuse's callbacks carry no pointer to it, so they read it from here.
pub(super) static LOADED: OnceLock<Sys> = OnceLock::new();

/// Loads the library at `path`, once. A failed load is not kept: the user may install macFUSE while the helper is running.
pub(super) fn load(path: &Path) -> Result<&'static Sys, ()> {
    if let Some(sys) = LOADED.get() {
        return Ok(sys);
    }
    // SAFETY: opening a library does not run Marlin code. The file is macFUSE's libfuse, or the open fails.
    let lib = unsafe { Library::new(path) }.map_err(|_| ())?;
    let sys = Sys {
        session_new: symbol(&lib, b"fuse_session_new_versioned\0")?,
        session_mount: symbol(&lib, b"fuse_session_mount\0")?,
        session_loop: symbol(&lib, b"fuse_session_loop\0")?,
        session_unmount: symbol(&lib, b"fuse_session_unmount\0")?,
        session_destroy: symbol(&lib, b"fuse_session_destroy\0")?,
        opt_free_args: symbol(&lib, b"fuse_opt_free_args\0")?,
        req_userdata: symbol(&lib, b"fuse_req_userdata\0")?,
        reply_err: symbol(&lib, b"fuse_reply_err\0")?,
        reply_none: symbol(&lib, b"fuse_reply_none\0")?,
        reply_entry: symbol(&lib, b"fuse_reply_entry\0")?,
        reply_attr: symbol(&lib, b"fuse_reply_attr\0")?,
        reply_open: symbol(&lib, b"fuse_reply_open\0")?,
        reply_data: symbol(&lib, b"fuse_reply_data\0")?,
        reply_statfs: symbol(&lib, b"fuse_reply_statfs\0")?,
        reply_write: symbol(&lib, b"fuse_reply_write\0")?,
        reply_create: symbol(&lib, b"fuse_reply_create\0")?,
        add_direntry: symbol(&lib, b"fuse_add_direntry\0")?,
        _lib: lib,
    };
    Ok(LOADED.get_or_init(|| sys))
}

#[cfg(test)]
mod tests {
    use std::mem::{offset_of, size_of};

    use super::*;

    #[test]
    fn layouts_match_the_headers_with_darwin_extensions_off() {
        assert_eq!(size_of::<FuseArgs>(), 24);
        assert_eq!(offset_of!(FuseArgs, argc), 0);
        assert_eq!(offset_of!(FuseArgs, argv), 8);
        assert_eq!(offset_of!(FuseArgs, allocated), 16);

        assert_eq!(size_of::<LibfuseVersion>(), 16);
        assert_eq!(offset_of!(LibfuseVersion, major), 0);
        assert_eq!(offset_of!(LibfuseVersion, minor), 4);
        assert_eq!(offset_of!(LibfuseVersion, hotfix), 8);
        assert_eq!(offset_of!(LibfuseVersion, flags), 12);

        assert_eq!(size_of::<FuseFileInfo>(), 64);
        assert_eq!(offset_of!(FuseFileInfo, flags), 0);
        assert_eq!(offset_of!(FuseFileInfo, bits), 4);
        assert_eq!(offset_of!(FuseFileInfo, padding2), 8);
        assert_eq!(offset_of!(FuseFileInfo, padding3), 12);
        assert_eq!(offset_of!(FuseFileInfo, fh), 16);
        assert_eq!(offset_of!(FuseFileInfo, lock_owner), 24);
        assert_eq!(offset_of!(FuseFileInfo, poll_events), 32);
        assert_eq!(offset_of!(FuseFileInfo, backing_id), 36);
        assert_eq!(offset_of!(FuseFileInfo, compat_flags), 40);
        assert_eq!(offset_of!(FuseFileInfo, reserved), 48);

        assert_eq!(size_of::<FuseBuf>(), 48);
        assert_eq!(offset_of!(FuseBuf, size), 0);
        assert_eq!(offset_of!(FuseBuf, flags), 8);
        assert_eq!(offset_of!(FuseBuf, mem), 16);
        assert_eq!(offset_of!(FuseBuf, fd), 24);
        assert_eq!(offset_of!(FuseBuf, pos), 32);
        assert_eq!(offset_of!(FuseBuf, mem_size), 40);

        assert_eq!(size_of::<FuseBufvec>(), 72);
        assert_eq!(offset_of!(FuseBufvec, count), 0);
        assert_eq!(offset_of!(FuseBufvec, idx), 8);
        assert_eq!(offset_of!(FuseBufvec, off), 16);
        assert_eq!(offset_of!(FuseBufvec, buf), 24);

        assert_eq!(size_of::<libc::stat>(), 144);
        assert_eq!(size_of::<libc::statvfs>(), 64);
        assert_eq!(size_of::<FuseEntryParam>(), 176);
        assert_eq!(offset_of!(FuseEntryParam, ino), 0);
        assert_eq!(offset_of!(FuseEntryParam, generation), 8);
        assert_eq!(offset_of!(FuseEntryParam, attr), 16);
        assert_eq!(offset_of!(FuseEntryParam, attr_timeout), 160);
        assert_eq!(offset_of!(FuseEntryParam, entry_timeout), 168);

        assert_eq!(size_of::<FuseLowlevelOps>(), 248);
        assert_eq!(offset_of!(FuseLowlevelOps, init), 0);
        assert_eq!(offset_of!(FuseLowlevelOps, destroy), 8);
        assert_eq!(offset_of!(FuseLowlevelOps, lookup), 16);
        assert_eq!(offset_of!(FuseLowlevelOps, forget), 24);
        assert_eq!(offset_of!(FuseLowlevelOps, getattr), 32);
        assert_eq!(offset_of!(FuseLowlevelOps, setattr), 40);
        assert_eq!(offset_of!(FuseLowlevelOps, readlink), 48);
        assert_eq!(offset_of!(FuseLowlevelOps, mknod), 56);
        assert_eq!(offset_of!(FuseLowlevelOps, mkdir), 64);
        assert_eq!(offset_of!(FuseLowlevelOps, unlink), 72);
        assert_eq!(offset_of!(FuseLowlevelOps, rmdir), 80);
        assert_eq!(offset_of!(FuseLowlevelOps, symlink), 88);
        assert_eq!(offset_of!(FuseLowlevelOps, rename), 96);
        assert_eq!(offset_of!(FuseLowlevelOps, link), 104);
        assert_eq!(offset_of!(FuseLowlevelOps, open), 112);
        assert_eq!(offset_of!(FuseLowlevelOps, read), 120);
        assert_eq!(offset_of!(FuseLowlevelOps, write), 128);
        assert_eq!(offset_of!(FuseLowlevelOps, flush), 136);
        assert_eq!(offset_of!(FuseLowlevelOps, release), 144);
        assert_eq!(offset_of!(FuseLowlevelOps, fsync), 152);
        assert_eq!(offset_of!(FuseLowlevelOps, opendir), 160);
        assert_eq!(offset_of!(FuseLowlevelOps, readdir), 168);
        assert_eq!(offset_of!(FuseLowlevelOps, releasedir), 176);
        assert_eq!(offset_of!(FuseLowlevelOps, fsyncdir), 184);
        assert_eq!(offset_of!(FuseLowlevelOps, statfs), 192);
        assert_eq!(offset_of!(FuseLowlevelOps, setxattr), 200);
        assert_eq!(offset_of!(FuseLowlevelOps, getxattr), 208);
        assert_eq!(offset_of!(FuseLowlevelOps, listxattr), 216);
        assert_eq!(offset_of!(FuseLowlevelOps, removexattr), 224);
        assert_eq!(offset_of!(FuseLowlevelOps, access), 232);
        assert_eq!(offset_of!(FuseLowlevelOps, create), 240);
    }
}
