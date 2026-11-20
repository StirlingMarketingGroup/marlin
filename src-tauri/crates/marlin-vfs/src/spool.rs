use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};

use crate::VfsError;

/// Where a mount keeps the bytes of files it changes, until they are uploaded (decision 0005).
pub struct Spool {
    /// An existing folder that only this mount writes in.
    pub folder: PathBuf,

    /// The most bytes the spool may hold at once. [`Spool::default_limit`] gives the default.
    pub limit: u64,

    /// Names the OS writes for itself (choice 14). A file under one is spooled and shown like any other, and never uploads. [`crate::finder_files`] is the macOS set. The adapter crate names the set for its OS: `marlin_fuse::stays_local`.
    pub stays_local: fn(&str) -> bool,
}

impl Spool {
    /// 10 GiB, or half the free space on the disk that holds `folder`, whichever is smaller (decision 0005, choice 11). Measured once, when it is called.
    pub fn default_limit(folder: &Path) -> io::Result<u64> {
        Ok(limit_for(free_bytes(folder)?))
    }
}

/// 10 GiB, or half of `free`, whichever is smaller.
fn limit_for(free: u64) -> u64 {
    const MOST: u64 = 10 * 1024 * 1024 * 1024;
    MOST.min(free / 2)
}

// Each call opens the spool file and closes it again. A copy of many small files would otherwise hold one open file per file until it uploads, and run out.

/// A new empty file at `path`. On Unix it is open to the user only. On Windows it takes the spool folder's permissions. The caller picks the next number when this is [`ErrorKind::AlreadyExists`]: a file left by an earlier mount keeps its number.
pub(crate) fn create_named(path: &Path) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).map(drop)
}

/// Up to `length` bytes from `offset`. Shorter only at the end of the file.
pub(crate) fn read_at(path: &Path, offset: u64, length: u32) -> io::Result<Vec<u8>> {
    let file = File::open(path)?;
    let mut buf = vec![0u8; length as usize];
    let mut filled = 0;
    while filled < buf.len() {
        match read_part(&file, &mut buf[filled..], offset + filled as u64) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    buf.truncate(filled);
    Ok(buf)
}

pub(crate) fn write_all_at(path: &Path, mut data: &[u8], mut offset: u64) -> io::Result<()> {
    let file = OpenOptions::new().write(true).open(path)?;
    while !data.is_empty() {
        match write_part(&file, data, offset) {
            Ok(0) => return Err(ErrorKind::WriteZero.into()),
            Ok(wrote) => {
                data = &data[wrote..];
                offset += wrote as u64;
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

pub(crate) fn set_len(path: &Path, size: u64) -> io::Result<()> {
    OpenOptions::new().write(true).open(path)?.set_len(size)
}

/// `{n}.record`, the record beside spool file `{n}`. A spool file's name is a number, with no extension.
pub(crate) fn record_path(spool_path: &Path) -> PathBuf {
    spool_path.with_extension("record")
}

/// Writes `bytes` as the record beside `spool_path`. A crash mid-write leaves the old record or the new one.
pub(crate) fn write_record(spool_path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temporary = spool_path.with_extension("record.new");
    {
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(&temporary)?.write_all(bytes)?;
    }
    std::fs::rename(temporary, record_path(spool_path))
}

/// Puts the spool file, its record, and (on Unix) their folder on the disk. A file a confirmed upload already deleted is not an error.
pub(crate) fn sync_to_disk(spool_path: &Path) -> io::Result<()> {
    // Windows flushes a file only through a handle that can write.
    for path in [spool_path, &record_path(spool_path)] {
        match OpenOptions::new().write(true).open(path) {
            Ok(file) => file.sync_all()?,
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    // A folder has no sync on Windows.
    #[cfg(unix)]
    if let Some(folder) = spool_path.parent() {
        File::open(folder)?.sync_all()?;
    }
    Ok(())
}

pub(crate) fn map_io(error: io::Error) -> VfsError {
    match error.kind() {
        ErrorKind::StorageFull | ErrorKind::QuotaExceeded => VfsError::NoSpace,
        _ => VfsError::Other(error.to_string()),
    }
}

#[cfg(unix)]
fn read_part(file: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(file, buf, offset)
}

#[cfg(unix)]
fn write_part(file: &File, data: &[u8], offset: u64) -> io::Result<usize> {
    std::os::unix::fs::FileExt::write_at(file, data, offset)
}

#[cfg(windows)]
fn read_part(file: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(file, buf, offset)
}

#[cfg(windows)]
fn write_part(file: &File, data: &[u8], offset: u64) -> io::Result<usize> {
    std::os::windows::fs::FileExt::seek_write(file, data, offset)
}

#[cfg(unix)]
fn free_bytes(folder: &Path) -> io::Result<u64> {
    let stat = rustix::fs::statvfs(folder)?;
    Ok(stat.f_bavail.saturating_mul(stat.f_frsize))
}

#[cfg(windows)]
fn free_bytes(folder: &Path) -> io::Result<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let mut wide: Vec<u16> = folder.as_os_str().encode_wide().collect();
    wide.push(0);
    let mut free = 0u64;
    // The caller's free bytes, which is the room this mount can actually take.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(free)
    }
}

#[cfg(test)]
mod tests {
    use super::{limit_for, Spool};

    const GIB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn the_default_limit_is_half_the_free_space_up_to_10_gib() {
        assert_eq!(limit_for(8 * GIB), 4 * GIB);
        assert_eq!(limit_for(30 * GIB), 10 * GIB);
        assert_eq!(limit_for(0), 0);
        let dir = tempfile::tempdir().unwrap();
        assert!(Spool::default_limit(dir.path()).unwrap() > 0);
    }
}
