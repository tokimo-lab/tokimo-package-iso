//! Stub for [`iso_reader`] on platforms where libudfread isn't packaged
//! (Windows + non-Linux/macOS Unix). Mirrors the internal surface of
//! `iso_reader.rs` so the rest of the crate compiles unchanged; all parsing
//! functions return `Err`.

use std::sync::Arc;

use crate::read_at::ReadAt;

#[derive(Debug, Clone)]
pub(crate) struct IsoExtent {
    pub offset: u64,
    pub length: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct M2tsFile {
    pub filename: String,
    pub size: u64,
    pub extents: Vec<IsoExtent>,
}

pub(crate) fn find_m2ts_files(_reader: Arc<dyn ReadAt>) -> Result<Vec<M2tsFile>, String> {
    Err("Blu-ray ISO reading not supported on this platform yet".into())
}

pub(crate) fn select_main_m2ts(files: &[M2tsFile]) -> Option<&M2tsFile> {
    files.iter().max_by_key(|f| f.size)
}
