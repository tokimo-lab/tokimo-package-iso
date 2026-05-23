//! Blu-ray ISO reading + M2TS extraction for remote VFS sources.
//!
//! Split out of `rust-server::apps::video::handlers::playback` so the main
//! server (notably the ffprobe queue handler) and a future externalized
//! `tokimo-app-video` can both depend on it without a cyclic dependency.
//!
//! # Public API
//!
//! * [`IsoMeta`] / [`IsoExtentJson`] — serializable M2TS location metadata
//! * [`parse_iso_m2ts`] / [`parse_iso_m2ts_with`] — parse a Blu-ray ISO and
//!   return the main M2TS location
//! * [`build_iso_m2ts_input`] / [`build_iso_m2ts_input_with`] — build an
//!   FFmpeg `DirectInput` for the main M2TS stream
//! * [`ReadAt`] — trait for any random-access reader; the `_with` variants
//!   accept this so callers without a `tokimo_vfs::Vfs` can still use the
//!   ISO core
//!
//! The two `_with` variants take `Arc<dyn ReadAt>` and form the real core;
//! the `Arc<Vfs>`-based wrappers are convenience adapters.

// libudfread is bundled inside tokimo-lib (transitive dep of libbluray).
// Supported on Linux, macOS, and Windows. Other platforms fall back to the stub.
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod iso_reader;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
#[path = "iso_reader_stub.rs"]
mod iso_reader;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod udfread_ffi;

mod meta;
mod read_at;

pub use meta::{
    IsoExtentJson, IsoMeta, build_iso_m2ts_input, build_iso_m2ts_input_with, parse_iso_m2ts, parse_iso_m2ts_with,
};
pub use read_at::ReadAt;
