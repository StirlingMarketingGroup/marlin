//! The Windows program of the mount-hello spike: a read-only WinFsp filesystem
//! with two files. A read of `slow.txt` returns `STATUS_PENDING`, and a tokio
//! task sends the answer three seconds later.

mod winfsp;

use std::ffi::{c_void, OsString};
use std::fs;
use std::io;
use std::mem;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::process::ExitCode;
use std::ptr;
use std::slice;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use tokio::runtime::{Handle, Runtime};
use tokio::task::JoinHandle;
use windows_sys::core::w;
use windows_sys::Win32::Foundation::{
    LocalFree, ERROR_ACCESS_DENIED, ERROR_WRITE_PROTECT, NTSTATUS, STATUS_BUFFER_OVERFLOW,
    STATUS_MEDIA_WRITE_PROTECTED, STATUS_OBJECT_NAME_NOT_FOUND, STATUS_PENDING, STATUS_SUCCESS,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Storage::FileSystem::{
    GetVolumeInformationW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL,
};

use winfsp::{DirInfo, FileInfo, FileSystem, Interface, TransactRsp, VolumeInfo, VolumeParams};

const USAGE: &str = "usage: mount-hello-windows <serve|self-test> <mountpoint>";

const HELLO: &str = "Hello from Marlin\n";
const SLOW: &str = "slow\n";
const SLOW_READ: Duration = Duration::from_secs(3);

// `open` gives WinFsp one of these as the file context. WinFsp hands it back
// on every later call for that open file.
const ROOT: usize = 1;
const HELLO_ID: usize = 2;
const SLOW_ID: usize = 3;

/// Lives in a `Box` from `mount` to `unmount`. `FileSystem::user_context` points at it.
struct State {
    rt: Handle,
    api: winfsp::Api,
    /// The one security descriptor that the root and both files answer with.
    security: Vec<u8>,
    /// True while a read of `slow.txt` is waiting.
    slow_pending: Arc<AtomicBool>,
    /// Callbacks that arrived while `slow_pending` was true.
    callbacks_while_slow: AtomicU64,
    /// Every slow read that was started. `unmount` waits for them.
    slow_reads: Mutex<Vec<JoinHandle<()>>>,
}

unsafe fn state<'a>(fs: *mut FileSystem) -> &'a State {
    &*(*fs).user_context.cast::<State>()
}

unsafe fn count_callback(fs: *mut FileSystem) {
    let state = state(fs);
    if state.slow_pending.load(Ordering::SeqCst) {
        state.callbacks_while_slow.fetch_add(1, Ordering::SeqCst);
    }
}

/// The NUL-terminated UTF-16 string that WinFsp passed to a callback.
unsafe fn from_wide(text: *const u16) -> String {
    let mut len = 0;
    while *text.add(len) != 0 {
        len += 1;
    }
    String::from_utf16_lossy(slice::from_raw_parts(text, len))
}

/// WinFsp names a file by its path inside the volume: `\` or `\hello.txt`.
/// Names compare without case, as Windows programs expect.
unsafe fn lookup(file_name: *const u16) -> Result<usize, NTSTATUS> {
    match from_wide(file_name).to_ascii_lowercase().as_str() {
        "\\" => Ok(ROOT),
        "\\hello.txt" => Ok(HELLO_ID),
        "\\slow.txt" => Ok(SLOW_ID),
        _ => Err(STATUS_OBJECT_NAME_NOT_FOUND),
    }
}

fn file_info(id: usize) -> FileInfo {
    let (file_attributes, file_size) = match id {
        ROOT => (FILE_ATTRIBUTE_DIRECTORY, 0),
        HELLO_ID => (FILE_ATTRIBUTE_NORMAL, HELLO.len() as u64),
        _ => (FILE_ATTRIBUTE_NORMAL, SLOW.len() as u64),
    };
    FileInfo {
        file_attributes,
        file_size,
        allocation_size: file_size.div_ceil(512) * 512,
        index_number: id as u64,
        ..FileInfo::default()
    }
}

/// Copies the part of `data` that a read at `offset` asks for. Returns the bytes copied.
unsafe fn copy_at(data: &str, offset: u64, length: u32, buffer: *mut c_void) -> u32 {
    let start = (offset as usize).min(data.len());
    let end = start.saturating_add(length as usize).min(data.len());
    ptr::copy_nonoverlapping(data[start..end].as_ptr(), buffer.cast(), end - start);
    (end - start) as u32
}

/// Both security callbacks answer with the one descriptor. WinFsp passes a null
/// `size` when it wants no descriptor.
unsafe fn copy_security(
    fs: *mut FileSystem,
    descriptor: *mut c_void,
    size: *mut usize,
) -> NTSTATUS {
    if size.is_null() {
        return STATUS_SUCCESS;
    }
    let security = &state(fs).security;
    let fits = security.len() <= *size;
    *size = security.len();
    if !fits {
        return STATUS_BUFFER_OVERFLOW;
    }
    ptr::copy_nonoverlapping(security.as_ptr(), descriptor.cast(), security.len());
    STATUS_SUCCESS
}

unsafe extern "C" fn get_volume_info(
    fs: *mut FileSystem,
    volume_info: *mut VolumeInfo,
) -> NTSTATUS {
    count_callback(fs);
    *volume_info = VolumeInfo {
        total_size: 1 << 30,
        free_size: 0,
        volume_label_length: 0,
        volume_label: [0; 32],
    };
    STATUS_SUCCESS
}

unsafe extern "C" fn get_security_by_name(
    fs: *mut FileSystem,
    file_name: *mut u16,
    file_attributes: *mut u32,
    security_descriptor: *mut c_void,
    security_descriptor_size: *mut usize,
) -> NTSTATUS {
    count_callback(fs);
    let id = match lookup(file_name) {
        Ok(id) => id,
        Err(status) => return status,
    };
    if !file_attributes.is_null() {
        *file_attributes = file_info(id).file_attributes;
    }
    copy_security(fs, security_descriptor, security_descriptor_size)
}

unsafe extern "C" fn create(
    fs: *mut FileSystem,
    _file_name: *mut u16,
    _create_options: u32,
    _granted_access: u32,
    _file_attributes: u32,
    _security_descriptor: *mut c_void,
    _allocation_size: u64,
    _file_context: *mut *mut c_void,
    _file_info: *mut FileInfo,
) -> NTSTATUS {
    count_callback(fs);
    STATUS_MEDIA_WRITE_PROTECTED
}

unsafe extern "C" fn open(
    fs: *mut FileSystem,
    file_name: *mut u16,
    _create_options: u32,
    _granted_access: u32,
    file_context: *mut *mut c_void,
    file_info: *mut FileInfo,
) -> NTSTATUS {
    count_callback(fs);
    let id = match lookup(file_name) {
        Ok(id) => id,
        Err(status) => return status,
    };
    *file_context = id as *mut c_void;
    *file_info = self::file_info(id);
    STATUS_SUCCESS
}

unsafe extern "C" fn overwrite(
    fs: *mut FileSystem,
    _file_context: *mut c_void,
    _file_attributes: u32,
    _replace_file_attributes: u8,
    _allocation_size: u64,
    _file_info: *mut FileInfo,
) -> NTSTATUS {
    count_callback(fs);
    STATUS_MEDIA_WRITE_PROTECTED
}

unsafe extern "C" fn read(
    fs: *mut FileSystem,
    file_context: *mut c_void,
    buffer: *mut c_void,
    offset: u64,
    length: u32,
    bytes_transferred: *mut u32,
) -> NTSTATUS {
    // WinFsp sends no read for a directory, so any other context is `hello.txt`.
    if file_context as usize != SLOW_ID {
        count_callback(fs);
        *bytes_transferred = copy_at(HELLO, offset, length, buffer);
        return STATUS_SUCCESS;
    }

    let state = state(fs);
    let hint = (*(*(state.api.get_operation_context)()).request).hint;
    let send_response = state.api.send_response;
    let slow_pending = Arc::clone(&state.slow_pending);
    // A raw pointer cannot move into the task. WinFsp keeps `fs` and `buffer`
    // valid until the response is sent, as memfs.cpp relies on for its slow reads.
    let (fs, buffer) = (fs as usize, buffer as usize);
    slow_pending.store(true, Ordering::SeqCst);
    let slow_read = state.rt.spawn(async move {
        tokio::time::sleep(SLOW_READ).await;
        // SAFETY: all-zero is a valid response, and zero is STATUS_SUCCESS.
        let mut response: TransactRsp = unsafe { mem::zeroed() };
        response.size = size_of::<TransactRsp>() as u16;
        response.kind = winfsp::TRANSACT_READ;
        response.hint = hint;
        // SAFETY: `unmount` waits for this task before it deletes the filesystem.
        unsafe {
            response.information = copy_at(SLOW, offset, length, buffer as *mut c_void);
            send_response(fs as *mut FileSystem, &mut response);
        }
        slow_pending.store(false, Ordering::SeqCst);
    });
    state.slow_reads.lock().unwrap().push(slow_read);
    STATUS_PENDING
}

unsafe extern "C" fn get_file_info(
    fs: *mut FileSystem,
    file_context: *mut c_void,
    file_info: *mut FileInfo,
) -> NTSTATUS {
    count_callback(fs);
    *file_info = self::file_info(file_context as usize);
    STATUS_SUCCESS
}

unsafe extern "C" fn get_security(
    fs: *mut FileSystem,
    _file_context: *mut c_void,
    security_descriptor: *mut c_void,
    security_descriptor_size: *mut usize,
) -> NTSTATUS {
    count_callback(fs);
    copy_security(fs, security_descriptor, security_descriptor_size)
}

/// One listing entry: the fixed part, then room for the longest name.
#[repr(C)]
struct DirEntry {
    info: DirInfo,
    name: [u16; 9],
}

// WinFsp sends a listing only for a directory, and the root is the only one.
unsafe extern "C" fn read_directory(
    fs: *mut FileSystem,
    _file_context: *mut c_void,
    _pattern: *mut u16,
    marker: *mut u16,
    buffer: *mut c_void,
    length: u32,
    bytes_transferred: *mut u32,
) -> NTSTATUS {
    count_callback(fs);
    let add_dir_info = state(fs).api.add_dir_info;
    // A marker is the last name of the call before. WinFsp wants the names that sort after it.
    let marker = (!marker.is_null()).then(|| from_wide(marker).to_ascii_lowercase());
    for (id, name) in [(HELLO_ID, "hello.txt"), (SLOW_ID, "slow.txt")] {
        if marker.as_deref().is_some_and(|marker| name <= marker) {
            continue;
        }
        let mut entry = DirEntry {
            info: DirInfo {
                size: (size_of::<DirInfo>() + name.len() * 2) as u16,
                file_info: file_info(id),
                padding: [0; 3],
            },
            name: [0; 9],
        };
        for (slot, unit) in entry.name.iter_mut().zip(name.encode_utf16()) {
            *slot = unit;
        }
        let entry = ptr::addr_of_mut!(entry).cast::<DirInfo>();
        if add_dir_info(entry, buffer, length, bytes_transferred) == 0 {
            return STATUS_SUCCESS;
        }
    }
    add_dir_info(ptr::null_mut(), buffer, length, bytes_transferred);
    STATUS_SUCCESS
}

/// WinFsp needs `create` and `overwrite` before it will open any file. For a
/// null write, rename, or metadata slot it answers `STATUS_INVALID_DEVICE_REQUEST`.
/// A delete is the exception: with no delete slot it reports success and deletes nothing.
static INTERFACE: Interface = Interface {
    get_volume_info: Some(get_volume_info),
    get_security_by_name: Some(get_security_by_name),
    create: Some(create),
    open: Some(open),
    overwrite: Some(overwrite),
    read: Some(read),
    get_file_info: Some(get_file_info),
    get_security: Some(get_security),
    read_directory: Some(read_directory),
    set_volume_label: None,
    cleanup: None,
    close: None,
    write: None,
    flush: None,
    set_basic_info: None,
    set_file_size: None,
    can_delete: None,
    rename: None,
    set_security: None,
    rest: [None; 45],
};

fn mount(rt: &Runtime, mountpoint: &Path) -> Result<*mut FileSystem, String> {
    // WinFsp creates the mount directory itself and deletes it at unmount.
    if mountpoint.exists() {
        return Err(format!("mount: {} already exists", mountpoint.display()));
    }
    let api = winfsp::load().map_err(|err| format!("mount: {err}"))?;

    // Full access for SYSTEM, administrators, and everyone, as memfs.cpp gives its root.
    let mut descriptor = ptr::null_mut();
    let mut descriptor_len = 0;
    // SAFETY: the string is valid SDDL, and both out pointers are live.
    let converted = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            w!("O:BAG:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;FA;;;WD)"),
            SDDL_REVISION_1,
            &mut descriptor,
            &mut descriptor_len,
        )
    };
    if converted == 0 {
        return Err(format!(
            "mount: security descriptor: {}",
            io::Error::last_os_error()
        ));
    }
    // SAFETY: Windows allocated `descriptor_len` bytes at `descriptor`.
    let security =
        unsafe { slice::from_raw_parts(descriptor.cast::<u8>(), descriptor_len as usize) }.to_vec();
    // SAFETY: the descriptor came from LocalAlloc and nothing else points at it.
    unsafe { LocalFree(descriptor) };

    // SAFETY: all-zero is a valid VolumeParams.
    let mut params: VolumeParams = unsafe { mem::zeroed() };
    params.version = size_of::<VolumeParams>() as u16;
    params.sector_size = 512;
    params.sectors_per_allocation_unit = 1;
    params.max_component_length = 255;
    // `file_info_timeout` stays 0, so WinFsp caches no file information and
    // every call in the `concurrent` step reaches this process.
    //
    // READ_ONLY_VOLUME only labels the volume. WinFsp still passes each write
    // call on, so `create` and `overwrite` are what refuse it.
    params.flags = winfsp::CASE_PRESERVED_NAMES
        | winfsp::UNICODE_ON_DISK
        | winfsp::PERSISTENT_ACLS
        | winfsp::READ_ONLY_VOLUME;
    for (slot, unit) in params
        .file_system_name
        .iter_mut()
        .zip("marlin-hello".encode_utf16())
    {
        *slot = unit;
    }

    let mut fs = ptr::null_mut();
    // fsctl.h:52 `FSP_FSCTL_DISK_DEVICE_NAME`: a disk volume. A directory cannot
    // be the mount point of a network volume.
    // SAFETY: every pointer is live for the call, and INTERFACE lives forever.
    let status = unsafe { (api.create)(w!("WinFsp.Disk"), &params, &INTERFACE, &mut fs) };
    if status != STATUS_SUCCESS {
        return Err(format!(
            "mount: FspFileSystemCreate: NTSTATUS {status:#010X}"
        ));
    }
    let state = Box::new(State {
        rt: rt.handle().clone(),
        api,
        security,
        slow_pending: Arc::new(AtomicBool::new(false)),
        callbacks_while_slow: AtomicU64::new(0),
        slow_reads: Mutex::new(Vec::new()),
    });
    // SAFETY: `fs` is the filesystem WinFsp just made. No callback runs before the dispatcher starts.
    unsafe { (*fs).user_context = Box::into_raw(state).cast() };

    let mut path: Vec<u16> = mountpoint.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: `path` is NUL-terminated, and WinFsp copies it.
    let status = unsafe { (api.set_mount_point)(fs, path.as_mut_ptr()) };
    if status != STATUS_SUCCESS {
        unmount(fs);
        return Err(format!(
            "mount: FspFileSystemSetMountPoint: NTSTATUS {status:#010X}"
        ));
    }
    // One dispatcher thread: if a slow read held it, the `concurrent` step would fail.
    // SAFETY: `fs` is live and its `user_context` is set.
    let status = unsafe { (api.start_dispatcher)(fs, 1) };
    if status != STATUS_SUCCESS {
        unmount(fs);
        return Err(format!(
            "mount: FspFileSystemStartDispatcher: NTSTATUS {status:#010X}"
        ));
    }
    Ok(fs)
}

/// Stops the dispatcher, lets every slow read send its answer, then deletes
/// the filesystem, which removes the mount point. memfs.cpp waits for its slow
/// reads at the same point.
fn unmount(fs: *mut FileSystem) {
    // SAFETY: `fs` came from `mount`, which set `user_context` to a leaked `Box<State>`.
    // After the dispatcher stops and the slow reads end, nothing else uses either.
    unsafe {
        let state = (*fs).user_context.cast::<State>();
        ((*state).api.stop_dispatcher)(fs);
        let slow_reads = mem::take(&mut *(*state).slow_reads.lock().unwrap());
        for slow_read in slow_reads {
            let _ = (*state).rt.block_on(slow_read);
        }
        ((*state).api.delete)(fs);
        drop(Box::from_raw(state));
    }
}

fn serve(rt: &Runtime, mountpoint: &Path) -> Result<(), String> {
    let fs = mount(rt, mountpoint)?;
    println!("mounted {}", mountpoint.display());
    let signaled = rt.block_on(tokio::signal::ctrl_c());
    unmount(fs);
    signaled.map_err(|err| format!("signal: {err}"))
}

fn dir_names(dir: &Path) -> io::Result<Vec<OsString>> {
    let mut names = Vec::new();
    for entry in fs::read_dir(dir)? {
        names.push(entry?.file_name());
    }
    names.sort();
    Ok(names)
}

/// Runs the checks against the live mount through ordinary OS calls.
/// Stops at the first failure and returns `<step>: <reason>`.
fn check_mounted(mountpoint: &Path, state: &State) -> Result<(), String> {
    // A directory that lists is not proof. The volume at the path must be ours.
    // GetVolumeInformationW wants the path to end with a backslash, which `join("")` adds.
    let root: Vec<u16> = mountpoint
        .join("")
        .as_os_str()
        .encode_wide()
        .chain([0])
        .collect();
    let mut file_system_name = [0u16; 32];
    // SAFETY: `root` is NUL-terminated, and the one buffer passed is as long as the length says.
    let found = unsafe {
        GetVolumeInformationW(
            root.as_ptr(),
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            file_system_name.as_mut_ptr(),
            file_system_name.len() as u32,
        )
    };
    if found == 0 {
        return Err(format!("mount: {}", io::Error::last_os_error()));
    }
    let file_system_name = String::from_utf16_lossy(&file_system_name);
    if file_system_name.trim_end_matches('\0') != "marlin-hello" {
        return Err(format!("mount: the file system is {file_system_name:?}"));
    }
    println!("PASS mount");

    let names = dir_names(mountpoint).map_err(|err| format!("list: {err}"))?;
    if names != ["hello.txt", "slow.txt"] {
        return Err(format!("list: {names:?}"));
    }
    println!("PASS list");

    let hello_path = mountpoint.join("hello.txt");
    let hello = fs::read_to_string(&hello_path).map_err(|err| format!("read: {err}"))?;
    if hello != HELLO {
        return Err(format!("read: {hello:?}"));
    }
    println!("PASS read");

    let slow_path = mountpoint.join("slow.txt");
    let slow_started = Instant::now();
    let slow_reader = thread::spawn(move || fs::read_to_string(slow_path));
    while !state.slow_pending.load(Ordering::SeqCst) {
        if slow_started.elapsed() > Duration::from_secs(2) {
            return Err("concurrent: the slow read never reached the filesystem".to_string());
        }
        thread::sleep(Duration::from_millis(5));
    }
    let fast_started = Instant::now();
    dir_names(mountpoint).map_err(|err| format!("concurrent: list: {err}"))?;
    fs::read_to_string(&hello_path).map_err(|err| format!("concurrent: read: {err}"))?;
    let fast = fast_started.elapsed();
    let callbacks = state.callbacks_while_slow.load(Ordering::SeqCst);
    if !state.slow_pending.load(Ordering::SeqCst) {
        return Err("concurrent: the slow read finished before the fast calls".to_string());
    }
    if callbacks == 0 {
        return Err(
            "concurrent: no callback reached the filesystem during the slow read".to_string(),
        );
    }
    if fast > Duration::from_secs(1) {
        return Err(format!("concurrent: fast calls took {fast:?}"));
    }
    let slow = slow_reader
        .join()
        .map_err(|_| "concurrent: the slow reader panicked".to_string())?
        .map_err(|err| format!("concurrent: slow read: {err}"))?;
    let slow_took = slow_started.elapsed();
    if slow != SLOW || slow_took < Duration::from_millis(2500) {
        return Err(format!(
            "concurrent: slow read gave {slow:?} in {slow_took:?}"
        ));
    }
    println!(
        "PASS concurrent: fast calls took {fast:?} with {callbacks} callbacks during the slow read ({slow_took:?})"
    );

    match fs::File::create(mountpoint.join("new.txt")) {
        Ok(_) => return Err("read-only: create succeeded".to_string()),
        Err(err) if err.raw_os_error() == Some(ERROR_WRITE_PROTECT as i32) => {
            println!("PASS read-only: ERROR_WRITE_PROTECT")
        }
        Err(err) if err.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32) => {
            println!("PASS read-only: ERROR_ACCESS_DENIED")
        }
        Err(err) => return Err(format!("read-only: {err}")),
    }
    Ok(())
}

fn self_test(rt: &Runtime, mountpoint: &Path) -> Result<(), String> {
    let fs = mount(rt, mountpoint)?;
    // SAFETY: the state lives until `unmount`.
    let checked = check_mounted(mountpoint, unsafe { state(fs) });
    // Unmount even when a check failed, so a failed run leaves no mount behind.
    unmount(fs);
    checked?;

    match mountpoint.try_exists() {
        Ok(false) => {}
        Ok(true) => return Err("unmount: the mount directory is still there".to_string()),
        Err(err) => return Err(format!("unmount: {err}")),
    }
    println!("PASS unmount");
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let run: fn(&Runtime, &Path) -> Result<(), String> =
        match (args.first().map(String::as_str), args.get(1)) {
            (Some("serve"), Some(_)) => serve,
            (Some("self-test"), Some(_)) => self_test,
            _ => {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
        };

    let rt = match Runtime::new() {
        Ok(rt) => rt,
        Err(err) => {
            println!("FAIL runtime: {err}");
            return ExitCode::FAILURE;
        }
    };
    match run(&rt, Path::new(&args[1])) {
        Ok(()) => ExitCode::SUCCESS,
        Err(reason) => {
            println!("FAIL {reason}");
            ExitCode::FAILURE
        }
    }
}
