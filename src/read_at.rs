//! Public [`ReadAt`] trait — decouples the ISO parser from any particular
//! storage layer. The synchronous signature mirrors the libudfread FFI bridge,
//! which performs blocking reads inside `tokio::task::block_in_place`.

use std::io;
use std::sync::Arc;

/// Random-access reader for an ISO image. Implementations should be cheap to
/// clone (wrap the inner state in `Arc`) and thread-safe.
///
/// Reads are synchronous: the libudfread C callback bridge calls into this
/// trait from within `block_in_place`, so async work must be reified by the
/// implementor (e.g. via a runtime handle + `block_on`).
pub trait ReadAt: Send + Sync {
    /// Read `len` bytes starting at byte `offset`. May return fewer bytes than
    /// requested at EOF.
    fn read_at(&self, offset: u64, len: usize) -> io::Result<Vec<u8>>;
    /// Total size in bytes of the underlying object.
    fn size(&self) -> u64;
}

/// Adapter that exposes a `tokimo_vfs::Vfs` file as a [`ReadAt`]. Used by the
/// convenience wrappers that accept `Arc<Vfs>` + path + size directly.
pub(crate) struct VfsReadAt {
    closure: tokimo_vfs::ReadAt,
    size: u64,
}

impl VfsReadAt {
    pub(crate) async fn new(vfs: Arc<tokimo_vfs::Vfs>, path: &str, size: u64) -> Self {
        let closure = vfs.to_read_at(std::path::Path::new(path)).await;
        Self { closure, size }
    }
}

impl ReadAt for VfsReadAt {
    fn read_at(&self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        (self.closure)(offset, len)
    }
    fn size(&self) -> u64 {
        self.size
    }
}
