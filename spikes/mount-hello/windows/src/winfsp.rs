//! Marlin's own bindings to the WinFsp 2.1 DLL: only the types and calls this spike uses.
//!
//! Transcribed from `inc/winfsp/fsctl.h` and `inc/winfsp/winfsp.h` at tag v2.1 of
//! github.com/winfsp/winfsp. Each item names its header line. The headers use
//! natural alignment and no `#pragma pack`, so `#[repr(C)]` gives the same layout
//! on ARM64 and x64. Nothing links against WinFsp: `load` finds the DLL through
//! the registry at run time.

use core::ffi::{c_void, CStr};
use core::mem::{offset_of, size_of};
use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::Path;

use windows_sys::core::w;
use windows_sys::Win32::Foundation::{ERROR_SUCCESS, HMODULE, NTSTATUS};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};

/// fsctl.h:160 `FspFsctlTransactReadKind`, the sixth value of an enum that starts at 0.
pub const TRANSACT_READ: u32 = 5;

/// fsctl.h:206-215: bits of `VolumeParams::flags`. The header packs them into
/// one `UINT32` bitfield, first field in the lowest bit.
pub const CASE_PRESERVED_NAMES: u32 = 1 << 1;
pub const UNICODE_ON_DISK: u32 = 1 << 2;
pub const PERSISTENT_ACLS: u32 = 1 << 3;
pub const READ_ONLY_VOLUME: u32 = 1 << 9;

/// fsctl.h:261 `FSP_FSCTL_VOLUME_PARAMS`.
#[repr(C)]
pub struct VolumeParams {
    pub version: u16,
    pub sector_size: u16,
    pub sectors_per_allocation_unit: u16,
    pub max_component_length: u16,
    pub volume_creation_time: u64,
    pub volume_serial_number: u32,
    pub transact_timeout: u32,
    pub irp_timeout: u32,
    pub irp_capacity: u32,
    pub file_info_timeout: u32,
    pub flags: u32,
    pub prefix: [u16; 192],
    pub file_system_name: [u16; 16],
    /// fsctl.h:239-254: the version 1 flags, timeouts, control code, and reserved
    /// fields. The spike leaves them zero.
    pub version_1: [u64; 6],
}
const _: () = {
    assert!(size_of::<VolumeParams>() == 504); // fsctl.h:266
    assert!(offset_of!(VolumeParams, volume_serial_number) == 16);
    assert!(offset_of!(VolumeParams, file_info_timeout) == 32);
    assert!(offset_of!(VolumeParams, flags) == 36);
    assert!(offset_of!(VolumeParams, file_system_name) == 424);
};

/// fsctl.h:268 `FSP_FSCTL_VOLUME_INFO`.
#[repr(C)]
pub struct VolumeInfo {
    pub total_size: u64,
    pub free_size: u64,
    pub volume_label_length: u16,
    pub volume_label: [u16; 32],
}
const _: () = assert!(size_of::<VolumeInfo>() == 88); // fsctl.h:275

/// fsctl.h:277 `FSP_FSCTL_FILE_INFO`.
#[repr(C)]
#[derive(Default)]
pub struct FileInfo {
    pub file_attributes: u32,
    pub reparse_tag: u32,
    pub allocation_size: u64,
    pub file_size: u64,
    pub creation_time: u64,
    pub last_access_time: u64,
    pub last_write_time: u64,
    pub change_time: u64,
    pub index_number: u64,
    pub hard_links: u32,
    pub ea_size: u32,
}
const _: () = assert!(size_of::<FileInfo>() == 72); // fsctl.h:291

/// fsctl.h:299 `FSP_FSCTL_DIR_INFO`. The UTF-16 file name follows the struct.
#[repr(C)]
pub struct DirInfo {
    /// Bytes in this struct plus the name.
    pub size: u16,
    pub file_info: FileInfo,
    /// fsctl.h:303: a union of `UINT64 NextOffset` and `UINT8 Padding[24]`.
    pub padding: [u64; 3],
}
const _: () = {
    assert!(size_of::<DirInfo>() == 104); // fsctl.h:311
    assert!(offset_of!(DirInfo, file_info) == 8);
};

/// fsctl.h:341 `FSP_FSCTL_TRANSACT_REQ`: only its first fields. WinFsp owns the
/// request, and the spike reads nothing past `hint`.
#[repr(C)]
pub struct TransactReq {
    pub version: u16,
    pub size: u16,
    pub kind: u32,
    /// Names the request. The response to a pending read carries it back.
    pub hint: u64,
}
const _: () = assert!(offset_of!(TransactReq, hint) == 8);

/// fsctl.h:541 `FSP_FSCTL_TRANSACT_RSP`.
#[repr(C)]
pub struct TransactRsp {
    pub version: u16,
    pub size: u16,
    pub kind: u32,
    pub hint: u64,
    /// fsctl.h:549 `IoStatus.Information`: for a read, the bytes read.
    pub information: u32,
    /// fsctl.h:550 `IoStatus.Status`.
    pub status: NTSTATUS,
    /// fsctl.h:552: a union of one struct per kind of request. A read response leaves it zero.
    pub by_kind: [u64; 13],
}
const _: () = {
    // No assert in the header. clang gives 128 for the header's struct on ARM64 and x64.
    assert!(size_of::<TransactRsp>() == 128);
    assert!(offset_of!(TransactRsp, hint) == 8);
    assert!(offset_of!(TransactRsp, information) == 16);
    assert!(offset_of!(TransactRsp, by_kind) == 24);
};

/// winfsp.h:1122 `FSP_FILE_SYSTEM_OPERATION_CONTEXT`: only its first field.
/// WinFsp owns the context.
#[repr(C)]
pub struct OperationContext {
    pub request: *mut TransactReq,
}

/// winfsp.h:1096 `FSP_FILE_SYSTEM`: only its first fields. WinFsp allocates the
/// struct, and the spike touches nothing past `user_context`.
#[repr(C)]
pub struct FileSystem {
    pub version: u16,
    /// Free for the filesystem's own use. The spike keeps its state here.
    pub user_context: *mut c_void,
}
const _: () = assert!(offset_of!(FileSystem, user_context) == 8);

/// A callback the spike does not implement. Every slot is one function pointer.
pub type Unused = Option<unsafe extern "C" fn()>;

/// winfsp.h:193 `FSP_FILE_SYSTEM_INTERFACE`: the callbacks WinFsp calls, in the
/// header's order. `file_context` is the value `open` stored for the file.
#[repr(C)]
pub struct Interface {
    /// winfsp.h:206
    pub get_volume_info:
        Option<unsafe extern "C" fn(fs: *mut FileSystem, volume_info: *mut VolumeInfo) -> NTSTATUS>,
    pub set_volume_label: Unused,
    /// winfsp.h:252. `file_attributes`, `security_descriptor`, and its size may each be null.
    pub get_security_by_name: Option<
        unsafe extern "C" fn(
            fs: *mut FileSystem,
            file_name: *mut u16,
            file_attributes: *mut u32,
            security_descriptor: *mut c_void,
            security_descriptor_size: *mut usize,
        ) -> NTSTATUS,
    >,
    /// winfsp.h:291
    pub create: Option<
        unsafe extern "C" fn(
            fs: *mut FileSystem,
            file_name: *mut u16,
            create_options: u32,
            granted_access: u32,
            file_attributes: u32,
            security_descriptor: *mut c_void,
            allocation_size: u64,
            file_context: *mut *mut c_void,
            file_info: *mut FileInfo,
        ) -> NTSTATUS,
    >,
    /// winfsp.h:322
    pub open: Option<
        unsafe extern "C" fn(
            fs: *mut FileSystem,
            file_name: *mut u16,
            create_options: u32,
            granted_access: u32,
            file_context: *mut *mut c_void,
            file_info: *mut FileInfo,
        ) -> NTSTATUS,
    >,
    /// winfsp.h:345. `BOOLEAN` is one byte.
    pub overwrite: Option<
        unsafe extern "C" fn(
            fs: *mut FileSystem,
            file_context: *mut c_void,
            file_attributes: u32,
            replace_file_attributes: u8,
            allocation_size: u64,
            file_info: *mut FileInfo,
        ) -> NTSTATUS,
    >,
    pub cleanup: Unused,
    pub close: Unused,
    /// winfsp.h:441. May return `STATUS_PENDING` and answer later with `Api::send_response`.
    pub read: Option<
        unsafe extern "C" fn(
            fs: *mut FileSystem,
            file_context: *mut c_void,
            buffer: *mut c_void,
            offset: u64,
            length: u32,
            bytes_transferred: *mut u32,
        ) -> NTSTATUS,
    >,
    pub write: Unused,
    pub flush: Unused,
    /// winfsp.h:507
    pub get_file_info: Option<
        unsafe extern "C" fn(
            fs: *mut FileSystem,
            file_context: *mut c_void,
            file_info: *mut FileInfo,
        ) -> NTSTATUS,
    >,
    pub set_basic_info: Unused,
    pub set_file_size: Unused,
    pub can_delete: Unused,
    pub rename: Unused,
    /// winfsp.h:652
    pub get_security: Option<
        unsafe extern "C" fn(
            fs: *mut FileSystem,
            file_context: *mut c_void,
            security_descriptor: *mut c_void,
            security_descriptor_size: *mut usize,
        ) -> NTSTATUS,
    >,
    pub set_security: Unused,
    /// winfsp.h:703
    pub read_directory: Option<
        unsafe extern "C" fn(
            fs: *mut FileSystem,
            file_context: *mut c_void,
            pattern: *mut u16,
            marker: *mut u16,
            buffer: *mut c_void,
            length: u32,
            bytes_transferred: *mut u32,
        ) -> NTSTATUS,
    >,
    /// winfsp.h:747-1092: `ResolveReparsePoints` through the 31 reserved slots.
    pub rest: [Unused; 45],
}
const _: () = {
    assert!(size_of::<Interface>() == 64 * size_of::<usize>()); // winfsp.h:1094
    assert!(offset_of!(Interface, read_directory) == 18 * size_of::<usize>());
};

/// The functions the spike calls in the WinFsp DLL.
#[derive(Clone, Copy)]
pub struct Api {
    /// winfsp.h:1157 `FspFileSystemCreate`
    pub create: unsafe extern "C" fn(
        device_path: *const u16,
        volume_params: *const VolumeParams,
        interface: *const Interface,
        fs: *mut *mut FileSystem,
    ) -> NTSTATUS,
    /// winfsp.h:1167 `FspFileSystemDelete`. It also removes the mount point.
    pub delete: unsafe extern "C" fn(fs: *mut FileSystem),
    /// winfsp.h:1187 `FspFileSystemSetMountPoint`
    pub set_mount_point:
        unsafe extern "C" fn(fs: *mut FileSystem, mount_point: *mut u16) -> NTSTATUS,
    /// winfsp.h:1212 `FspFileSystemStartDispatcher`
    pub start_dispatcher: unsafe extern "C" fn(fs: *mut FileSystem, thread_count: u32) -> NTSTATUS,
    /// winfsp.h:1219 `FspFileSystemStopDispatcher`
    pub stop_dispatcher: unsafe extern "C" fn(fs: *mut FileSystem),
    /// winfsp.h:1240 `FspFileSystemSendResponse`
    pub send_response: unsafe extern "C" fn(fs: *mut FileSystem, response: *mut TransactRsp),
    /// winfsp.h:1318 `FspFileSystemGetOperationContext`. Valid only inside a callback.
    pub get_operation_context: unsafe extern "C" fn() -> *mut OperationContext,
    /// winfsp.h:1566 `FspFileSystemAddDirInfo`. Returns 0 when the buffer is full.
    /// A null `dir_info` marks the end of the listing.
    pub add_dir_info: unsafe extern "C" fn(
        dir_info: *mut DirInfo,
        buffer: *mut c_void,
        length: u32,
        bytes_transferred: *mut u32,
    ) -> u8,
}

/// winfsp.h:2189 `FSP_DLLNAME`.
const DLL: &str = if cfg!(target_arch = "aarch64") {
    "winfsp-a64.dll"
} else {
    "winfsp-x64.dll"
};

/// Loads the WinFsp DLL from the install folder that WinFsp's installer wrote
/// to the registry (fsctl.h:73).
pub fn load() -> Result<Api, String> {
    let mut install_dir = [0u16; 260];
    let mut bytes = size_of_val(&install_dir) as u32;
    // SAFETY: the buffer is `bytes` long, and RRF_RT_REG_SZ makes Windows end the string with a NUL.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!("SOFTWARE\\WOW6432Node\\WinFsp"),
            w!("InstallDir"),
            RRF_RT_REG_SZ,
            core::ptr::null_mut(),
            install_dir.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if status != ERROR_SUCCESS {
        return Err("WinFsp is not installed".to_string());
    }
    let len = install_dir.iter().position(|&unit| unit == 0).unwrap();
    let install_dir = OsString::from_wide(&install_dir[..len]);
    let dll: Vec<u16> = Path::new(&install_dir)
        .join("bin")
        .join(DLL)
        .as_os_str()
        .encode_wide()
        .chain([0])
        .collect();
    // SAFETY: `dll` is a NUL-terminated path.
    let module = unsafe { LoadLibraryW(dll.as_ptr()) };
    if module.is_null() {
        return Err("WinFsp is not installed".to_string());
    }
    Ok(Api {
        create: export(module, c"FspFileSystemCreate")?,
        delete: export(module, c"FspFileSystemDelete")?,
        set_mount_point: export(module, c"FspFileSystemSetMountPoint")?,
        start_dispatcher: export(module, c"FspFileSystemStartDispatcher")?,
        stop_dispatcher: export(module, c"FspFileSystemStopDispatcher")?,
        send_response: export(module, c"FspFileSystemSendResponse")?,
        get_operation_context: export(module, c"FspFileSystemGetOperationContext")?,
        add_dir_info: export(module, c"FspFileSystemAddDirInfo")?,
    })
}

/// Finds one exported function. `T` is the function pointer type from `Api`.
fn export<T>(module: HMODULE, name: &CStr) -> Result<T, String> {
    const { assert!(size_of::<T>() == size_of::<usize>()) };
    // SAFETY: `module` came from LoadLibraryW and `name` is NUL-terminated.
    let Some(found) = (unsafe { GetProcAddress(module, name.as_ptr().cast()) }) else {
        return Err(format!("WinFsp is missing {}", name.to_string_lossy()));
    };
    // SAFETY: `found` and `T` are both one function pointer, and the header gives
    // the export the signature that `Api` declares for it.
    Ok(unsafe { core::mem::transmute_copy(&found) })
}
