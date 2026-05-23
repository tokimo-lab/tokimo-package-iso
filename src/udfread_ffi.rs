//! Raw FFI bindings to libudfread (VLC/libbluray UDF 2.50 reader).
//!
//! libudfread is installed at `/usr/lib/x86_64-linux-gnu/libudfread.so`.
//! Headers: `/usr/include/udfread/udfread.h`, `/usr/include/udfread/blockinput.h`.
//!
//! The key extension point is `udfread_block_input`: a vtable struct that libudfread
//! calls to read raw 2048-byte sectors from the image source.  We overlay it with
//! `VfsBlockInput` (same layout, extra Rust fields after the vtable) so the C
//! callbacks can reach the VFS closure via a plain pointer cast.

#![cfg(any(target_os = "linux", target_os = "macos"))]
#![allow(unsafe_code)]

use std::ffi::c_int;
use std::os::raw::{c_char, c_uint, c_void};

// ── C struct / enum definitions ───────────────────────────────────────────────

/// Vtable that libudfread calls to read raw 2048-byte sectors.
/// Must be `#[repr(C)]` and match the ABI of `struct udfread_block_input`.
#[repr(C)]
pub struct UdfreadBlockInput {
    pub close: Option<unsafe extern "C" fn(*mut UdfreadBlockInput) -> c_int>,
    pub read: Option<
        unsafe extern "C" fn(
            *mut UdfreadBlockInput,
            u32,         // lba
            *mut c_void, // buf
            u32,         // nblocks
            c_int,       // flags
        ) -> c_int,
    >,
    pub size: Option<unsafe extern "C" fn(*mut UdfreadBlockInput) -> c_uint>,
}

/// Directory entry returned by `udfread_readdir`.
#[repr(C)]
pub struct UdfDirent {
    pub d_type: c_uint, // UDF_DT_DIR=1, UDF_DT_REG=2
    pub d_name: *const c_char,
}

pub const UDF_DT_REG: c_uint = 2;

/// Opaque handle for a mounted UDF volume.
pub enum Udfread {}
/// Opaque handle for an open directory stream.
pub enum UdfDir {}
/// Opaque handle for an open file.
pub enum UdfFile {}

// ── extern "C" declarations ───────────────────────────────────────────────────

unsafe extern "C" {
    pub fn udfread_init() -> *mut Udfread;
    pub fn udfread_open_input(udf: *mut Udfread, input: *mut UdfreadBlockInput) -> c_int;
    pub fn udfread_close(udf: *mut Udfread);

    pub fn udfread_opendir(udf: *mut Udfread, path: *const c_char) -> *mut UdfDir;
    pub fn udfread_readdir(dir: *mut UdfDir, entry: *mut UdfDirent) -> *mut UdfDirent;
    pub fn udfread_closedir(dir: *mut UdfDir);

    pub fn udfread_file_open(udf: *mut Udfread, path: *const c_char) -> *mut UdfFile;
    pub fn udfread_file_size(file: *mut UdfFile) -> i64;
    /// Convert file-local block number → absolute LBA on the disc. Returns 0 on error.
    pub fn udfread_file_lba(file: *mut UdfFile, file_block: u32) -> u32;
    pub fn udfread_file_close(file: *mut UdfFile);
}

// ── VfsBlockInput — overlay struct for VFS-backed block reads ─────────────────
//
// C code receives `*mut UdfreadBlockInput`; we cast it to `*mut VfsBlockInput`.
// This is valid because `base` is the first field and both are `#[repr(C)]`.

/// Extended block-input that stores a Rust VFS closure after the C vtable.
#[repr(C)]
pub struct VfsBlockInput {
    /// MUST be first — C code casts `*mut VfsBlockInput` to `*mut UdfreadBlockInput`.
    pub base: UdfreadBlockInput,
    /// Synchronous read closure: `(byte_offset, len) → Vec<u8>`.
    pub read_fn: Box<dyn Fn(u64, usize) -> std::io::Result<Vec<u8>> + Send>,
    /// ISO image size in 2048-byte sectors (for the `size` callback).
    pub iso_size_blocks: u32,
}

// SAFETY: VfsBlockInput is only ever accessed from one thread at a time
// (either within block_in_place or spawn_blocking).
unsafe impl Send for VfsBlockInput {}

/// C `read` callback — called by libudfread to read `nblocks` sectors from LBA `lba`.
pub unsafe extern "C" fn vfs_read_callback(
    input: *mut UdfreadBlockInput,
    lba: u32,
    buf: *mut c_void,
    nblocks: u32,
    _flags: c_int,
) -> c_int {
    let vfs = unsafe { &*input.cast::<VfsBlockInput>() };
    let offset = u64::from(lba) * 2048;
    let size = nblocks as usize * 2048;

    match (vfs.read_fn)(offset, size) {
        Ok(data) => {
            let n = data.len().min(size);
            if n > 0 {
                unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), buf.cast::<u8>(), n) };
            }
            (n / 2048) as c_int
        }
        Err(_) => -1,
    }
}

/// C `size` callback — returns total disc size in sectors.
pub unsafe extern "C" fn vfs_size_callback(input: *mut UdfreadBlockInput) -> c_uint {
    let vfs = unsafe { &*input.cast::<VfsBlockInput>() };
    vfs.iso_size_blocks
}
