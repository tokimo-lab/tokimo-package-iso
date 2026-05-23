//! Serializable M2TS location metadata + helpers that construct an
//! `FFmpeg` `DirectInput` for remote Blu-ray ISOs.

use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::iso_reader;
use crate::read_at::{ReadAt, VfsReadAt};

/// Serializable M2TS location info stored in `video_files.iso_meta`.
/// Written during ffprobe scan; read during playback to skip UDF re-parsing.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IsoMeta {
    pub filename: String,
    pub size: u64,
    pub extents: Vec<IsoExtentJson>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IsoExtentJson {
    pub offset: u64,
    pub length: u64,
}

impl IsoMeta {
    pub(crate) fn from_m2ts(m: &iso_reader::M2tsFile) -> Self {
        Self {
            filename: m.filename.clone(),
            size: m.size,
            extents: m
                .extents
                .iter()
                .map(|e| IsoExtentJson {
                    offset: e.offset,
                    length: e.length,
                })
                .collect(),
        }
    }
}

// ── Core API (`_with` variants accept any `ReadAt`) ──────────────────────────

/// UDF parse + main M2TS selection from any random-access reader.
///
/// Prefer this when you already have an `Arc<dyn ReadAt>`; otherwise see the
/// `Arc<Vfs>` convenience wrapper [`parse_iso_m2ts`].
pub async fn parse_iso_m2ts_with(reader: Arc<dyn ReadAt>) -> Result<IsoMeta, String> {
    let m2ts_files = iso_reader::find_m2ts_files(reader).map_err(|e| format!("UDF parse failed: {e}"))?;

    if m2ts_files.is_empty() {
        return Err("No M2TS files found in BDMV/STREAM/ — not a Blu-ray ISO?".to_string());
    }

    let main =
        iso_reader::select_main_m2ts(&m2ts_files).ok_or_else(|| "Could not select main M2TS from ISO".to_string())?;

    Ok(IsoMeta::from_m2ts(main))
}

/// Build a `DirectInput` for a Blu-ray ISO accessed through any
/// `Arc<dyn ReadAt>`. When `iso_meta` is `None`, the UDF filesystem is parsed
/// live; otherwise the cached metadata is used (no UDF re-parse).
pub async fn build_iso_m2ts_input_with(
    reader: Arc<dyn ReadAt>,
    iso_meta: Option<&IsoMeta>,
    subtitle_tap: Option<tokio::sync::mpsc::Sender<(bytes::Bytes, u64)>>,
) -> Result<Arc<tokimo_package_ffmpeg::DirectInput>, String> {
    // UDF parsing is expensive (~1s over SMB). The scan phase already parsed
    // the UDF and stored the M2TS location in `video_files.iso_meta`. Use it
    // when available; only fall back to live UDF parse for un-scanned files.
    let iso_meta_owned: IsoMeta = if let Some(m) = iso_meta {
        debug!("[ISO] Using pre-scanned M2TS info from iso_meta (no UDF re-parse)");
        m.clone()
    } else {
        warn!("[ISO] iso_meta not in DB, falling back to live UDF parse (re-scan to fix)");
        parse_iso_m2ts_with(reader.clone()).await?
    };

    info!(
        "[ISO] Main M2TS: {} ({:.1} GB, {} extent(s))",
        iso_meta_owned.filename,
        iso_meta_owned.size as f64 / 1_073_741_824.0,
        iso_meta_owned.extents.len(),
    );
    for (i, ext) in iso_meta_owned.extents.iter().enumerate() {
        debug!(
            "[ISO]   extent {i}: ISO offset={} size={}MB",
            ext.offset,
            ext.length / 1_048_576,
        );
    }

    Ok(build_direct_input_from_meta(reader, iso_meta_owned, subtitle_tap))
}

// ── Vfs convenience wrappers (preserve old call sites) ───────────────────────

/// UDF parse + main M2TS selection for a VFS-backed ISO file. Convenience
/// wrapper around [`parse_iso_m2ts_with`] for callers that already hold an
/// `Arc<Vfs>`.
pub async fn parse_iso_m2ts(vfs: &Arc<tokimo_vfs::Vfs>, iso_path: &str, file_size: u64) -> Result<IsoMeta, String> {
    let reader: Arc<dyn ReadAt> = Arc::new(VfsReadAt::new(vfs.clone(), iso_path, file_size).await);
    parse_iso_m2ts_with(reader).await
}

/// Build a `DirectInput` for a remote Blu-ray ISO by parsing the UDF filesystem
/// to locate the main M2TS stream within the ISO, then returning an AVIO reader
/// that maps M2TS-local byte offsets to the correct ranges within the ISO file.
///
/// Convenience wrapper around [`build_iso_m2ts_input_with`] for callers that
/// already hold an `Arc<Vfs>`.
pub async fn build_iso_m2ts_input(
    vfs: Arc<tokimo_vfs::Vfs>,
    file_path: &str,
    file_size: u64,
    iso_meta: Option<&IsoMeta>,
    subtitle_tap: Option<tokio::sync::mpsc::Sender<(bytes::Bytes, u64)>>,
) -> Result<Arc<tokimo_package_ffmpeg::DirectInput>, String> {
    let reader: Arc<dyn ReadAt> = Arc::new(VfsReadAt::new(vfs, file_path, file_size).await);
    build_iso_m2ts_input_with(reader, iso_meta, subtitle_tap).await
}

// ── Internals ────────────────────────────────────────────────────────────────

fn build_direct_input_from_meta(
    reader: Arc<dyn ReadAt>,
    iso_meta: IsoMeta,
    subtitle_tap: Option<tokio::sync::mpsc::Sender<(bytes::Bytes, u64)>>,
) -> Arc<tokimo_package_ffmpeg::DirectInput> {
    let m2ts_size = iso_meta.size;
    let filename = iso_meta.filename.clone();
    let extents = iso_meta.extents;

    let reader_for_read = reader;
    let input = tokimo_package_ffmpeg::DirectInput {
        read_at: Arc::new(move |m2ts_offset: u64, size: usize| {
            let result = read_from_m2ts_extents(&extents, m2ts_offset, size, |iso_offset, len| {
                reader_for_read.read_at(iso_offset, len)
            })?;
            if let Some(ref tx) = subtitle_tap {
                let _ = tx.try_send((bytes::Bytes::copy_from_slice(&result), m2ts_offset));
            }
            Ok(result)
        }),
        size: m2ts_size,
        filename_hint: Some(filename),
        readahead_bytes: Some(tokimo_package_ffmpeg::READAHEAD_HLS),
    };

    Arc::new(input)
}

/// Map a logical read `(m2ts_offset, size)` through a list of `IsoExtentJson`s
/// to physical ISO reads, concatenating the results into a single `Vec<u8>`.
///
/// `iso_read(iso_offset, len)` reads `len` bytes at absolute ISO position `iso_offset`.
fn read_from_m2ts_extents(
    extents: &[IsoExtentJson],
    m2ts_offset: u64,
    size: usize,
    iso_read: impl Fn(u64, usize) -> std::io::Result<Vec<u8>>,
) -> std::io::Result<Vec<u8>> {
    let mut result = Vec::with_capacity(size);
    let mut remaining = size as u64;
    let mut logical_pos = m2ts_offset;

    for ext in extents {
        if remaining == 0 {
            break;
        }
        // Does this extent cover any part of [logical_pos, logical_pos + remaining)?
        if logical_pos >= ext.length {
            // This extent is entirely before our read window — skip it.
            logical_pos -= ext.length;
            continue;
        }
        // Read starts at `logical_pos` within this extent.
        let ext_read_offset = logical_pos;
        let ext_read_len = (ext.length - ext_read_offset).min(remaining) as usize;
        let iso_offset = ext.offset + ext_read_offset;

        let chunk = iso_read(iso_offset, ext_read_len)?;
        result.extend_from_slice(&chunk);
        remaining -= chunk.len() as u64;
        logical_pos = 0; // consumed fully into next extent
    }

    Ok(result)
}
