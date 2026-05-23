//! Blu-ray ISO reader — locates M2TS files inside a UDF 2.50 disc image.
//!
//! Uses libudfread (the same C library used internally by libbluray and VLC) via FFI.
//! libudfread correctly handles UDF 2.50 Metadata Partitions that Blu-ray discs use,
//! which a hand-rolled Rust UDF parser cannot reliably handle.
//!
//! # Two playback paths
//!
//! * **Local ISO** → ffmpeg receives `bluray:/path/to/file.iso`; libbluray handles
//!   everything.  This module is *not* used for local files.
//! * **Remote ISO** (SMB / SFTP / S3 / …) → this module parses the UDF filesystem
//!   via a VFS read callback, locates the main M2TS stream, and returns its byte
//!   extents inside the ISO so ffmpeg can read it via the existing AVIO mechanism.

#![cfg(any(target_os = "linux", target_os = "macos"))]
#![allow(unsafe_code)]

use std::ffi::{CStr, CString};
use std::sync::Arc;
use tracing::{debug, warn};

use crate::read_at::ReadAt;
use crate::udfread_ffi::{
    UDF_DT_REG, UdfDirent, UdfreadBlockInput, VfsBlockInput, udfread_close, udfread_closedir, udfread_file_close,
    udfread_file_lba, udfread_file_open, udfread_file_size, udfread_init, udfread_open_input, udfread_opendir,
    udfread_readdir, vfs_read_callback, vfs_size_callback,
};

// ── Public types (unchanged — callers in playback.rs depend on these) ─────────

/// A contiguous byte range within the ISO image.
#[derive(Debug, Clone)]
pub(crate) struct IsoExtent {
    /// Byte offset from the start of the ISO file.
    pub offset: u64,
    /// Length in bytes.
    pub length: u64,
}

/// Location and size of a single M2TS stream file inside the ISO.
#[derive(Debug, Clone)]
pub(crate) struct M2tsFile {
    pub filename: String,
    /// Total file size in bytes.
    pub size: u64,
    /// Sorted, non-overlapping extents covering the file (usually just one).
    pub extents: Vec<IsoExtent>,
}

type UdfResult<T> = Result<T, String>;

// ── Public API ─────────────────────────────────────────────────────────────────

/// Find all M2TS files in `/BDMV/STREAM/` inside a Blu-ray ISO image.
///
/// `reader` is any random-access reader for the ISO; its `size()` is used by the
/// libudfread `size` callback to bound sector lookups.
///
/// Runs synchronous libudfread C calls inside `block_in_place` so it does not
/// block the async executor beyond what tokio expects for blocking I/O helpers.
pub(crate) fn find_m2ts_files(reader: Arc<dyn ReadAt>) -> UdfResult<Vec<M2tsFile>> {
    tokio::task::block_in_place(|| find_m2ts_files_sync(reader))
}

/// Select the main title M2TS: the largest file in BDMV/STREAM/.
///
/// The main feature is always the largest M2TS; menu clips and extras are small.
pub(crate) fn select_main_m2ts(files: &[M2tsFile]) -> Option<&M2tsFile> {
    files.iter().max_by_key(|f| f.size)
}

// ── Internal synchronous implementation ──────────────────────────────────────

fn find_m2ts_files_sync(reader: Arc<dyn ReadAt>) -> UdfResult<Vec<M2tsFile>> {
    let iso_size_blocks = (reader.size() / 2048) as u32;

    // Build a VfsBlockInput that wraps the random-access reader.
    let mut vfs_input = VfsBlockInput {
        base: UdfreadBlockInput {
            close: None,
            read: Some(vfs_read_callback),
            size: Some(vfs_size_callback),
        },
        read_fn: Box::new(move |offset, size| reader.read_at(offset, size)),
        iso_size_blocks,
    };

    let dir_path = CString::new("/BDMV/STREAM").map_err(|e| format!("invalid dir path: {e}"))?;

    // SAFETY: all pointers live for the duration of this function.
    unsafe {
        let udf = udfread_init();
        if udf.is_null() {
            return Err("udfread_init() returned NULL (out of memory?)".to_string());
        }

        let ret = udfread_open_input(udf, &raw mut vfs_input.base);
        if ret < 0 {
            udfread_close(udf);
            return Err(format!(
                "udfread_open_input failed (code {ret}): not a valid UDF disc image?"
            ));
        }

        debug!("[ISO] UDF volume opened via libudfread");

        let dir = udfread_opendir(udf, dir_path.as_ptr());
        if dir.is_null() {
            udfread_close(udf);
            return Err("BDMV/STREAM directory not found — not a Blu-ray ISO?".to_string());
        }

        let mut files = Vec::new();
        let mut entry = UdfDirent {
            d_type: 0,
            d_name: std::ptr::null(),
        };

        loop {
            let ep = udfread_readdir(dir, &raw mut entry);
            if ep.is_null() {
                break;
            }
            if entry.d_type != UDF_DT_REG {
                continue;
            }

            let name = CStr::from_ptr(entry.d_name).to_string_lossy().into_owned();
            if !name.to_ascii_lowercase().ends_with(".m2ts") {
                continue;
            }

            let file_path = format!("/BDMV/STREAM/{name}");
            let Ok(file_path_c) = CString::new(file_path) else {
                continue;
            };
            let file = udfread_file_open(udf, file_path_c.as_ptr());
            if file.is_null() {
                warn!("[ISO] Could not open {name}");
                continue;
            }

            let size = udfread_file_size(file);
            if size <= 0 {
                udfread_file_close(file);
                continue;
            }
            let size = size as u64;

            let extents = build_extents(file, size);
            debug!("[ISO] {name}: {:.2} GB, {} extent(s)", size as f64 / 1e9, extents.len());

            udfread_file_close(file);
            files.push(M2tsFile {
                filename: name,
                size,
                extents,
            });
        }

        udfread_closedir(dir);
        udfread_close(udf);

        Ok(files)
    }
}

/// Build ISO byte extents for a file using `udfread_file_lba`.
///
/// For Blu-ray main-title M2TS the file is almost always stored in a single
/// contiguous run.  We verify with a two-probe check (first + last block) and
/// only do a linear scan when the file is genuinely fragmented (extremely rare).
unsafe fn build_extents(file: *mut super::udfread_ffi::UdfFile, size: u64) -> Vec<IsoExtent> {
    let total_blocks = size.div_ceil(2048) as u32;
    if total_blocks == 0 {
        return vec![];
    }

    let first_lba = unsafe { udfread_file_lba(file, 0) };
    if first_lba == 0 {
        warn!("[ISO] udfread_file_lba returned 0 for block 0 — skipping file");
        return vec![];
    }

    // Single block — trivial.
    if total_blocks == 1 {
        return vec![IsoExtent {
            offset: u64::from(first_lba) * 2048,
            length: size,
        }];
    }

    let last_lba = unsafe { udfread_file_lba(file, total_blocks - 1) };

    // Fast path: single contiguous extent (typical for Blu-ray main M2TS).
    if last_lba == first_lba + total_blocks - 1 {
        return vec![IsoExtent {
            offset: u64::from(first_lba) * 2048,
            length: size,
        }];
    }

    // Slow path: fragmented — scan block by block.
    debug!(
        "[ISO] File is fragmented (first_lba={first_lba}, last_lba={last_lba}, \
         total_blocks={total_blocks}); scanning extents…"
    );

    let mut extents = Vec::new();
    let mut block = 0u32;

    while block < total_blocks {
        let lba = unsafe { udfread_file_lba(file, block) };
        if lba == 0 {
            block += 1;
            continue;
        }

        // Extend the run while LBAs are consecutive.
        let run_start_lba = lba;
        let mut run_len = 1u32;
        while block + run_len < total_blocks {
            let next = unsafe { udfread_file_lba(file, block + run_len) };
            if next != run_start_lba + run_len {
                break;
            }
            run_len += 1;
        }

        let byte_len = if block + run_len >= total_blocks {
            // Last extent — clamp to actual file size.
            size.saturating_sub(u64::from(block) * 2048)
        } else {
            u64::from(run_len) * 2048
        };

        extents.push(IsoExtent {
            offset: u64::from(run_start_lba) * 2048,
            length: byte_len,
        });
        block += run_len;
    }

    extents
}
