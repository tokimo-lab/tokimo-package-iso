# tokimo-package-iso

Blu-ray ISO reading and M2TS stream extraction for remote VFS-backed sources (SMB, SFTP, S3, etc.).

## Features

- **UDF 2.50 parsing**: Reads the UDF filesystem inside `.iso` disc images via `libudfread` FFI
- **M2TS extraction**: Locates the main video stream in `/BDMV/STREAM/` and builds FFmpeg-compatible `DirectInput`
- **VFS integration**: Works with any storage backend through the `tokimo-vfs` abstraction
- **Cached metadata**: `IsoMeta` can be serialized to DB and reused for playback without re-parsing
- **Platform support**: Linux and macOS (via `libudfread`), Windows returns stub errors

## Public API

```rust
use tokimo_package_iso::{IsoMeta, parse_iso_m2ts, build_iso_m2ts_input, ReadAt};

// Parse a Blu-ray ISO to get M2TS location metadata
let meta = parse_iso_m2ts(&vfs, "/path/to/movie.iso", file_size).await?;

// Build FFmpeg DirectInput for playback
let input = build_iso_m2ts_input(vfs, path, size, Some(&meta), None).await?;
```

## External Dependencies

- **libudfread**: System shared library for UDF filesystem parsing
  - Linux: `apt install libudfread-dev`
  - macOS: `brew install libudfread`

## Architecture

| Module | Purpose |
|--------|---------|
| `meta.rs` | `IsoMeta`, `parse_iso_m2ts`, `build_iso_m2ts_input` |
| `read_at.rs` | `ReadAt` trait for random-access readers |
| `iso_reader.rs` | `libudfread` FFI bindings (Linux/macOS) |
| `iso_reader_stub.rs` | Stub for unsupported platforms |
| `udfread_ffi.rs` | Raw C FFI definitions |
