use std::sync::Arc;

use crate::error::Result;

/// Shared handle to a UDX runtime, held by sockets and streams.
///
/// External code only holds `Arc<RuntimeHandle>` — all methods are `pub(crate)`.
pub struct RuntimeHandle {
    _private: (),
}

/// Owns and drives the UDX protocol runtime.
///
/// Sockets and streams are created through this runtime. In the native
/// implementation the runtime is lightweight — it relies on the ambient
/// tokio executor for async I/O.
pub struct UdxRuntime {
    handle: Arc<RuntimeHandle>,
}

impl UdxRuntime {
    /// Create a new owning runtime.
    pub fn new() -> Result<Self> {
        Ok(Self {
            handle: Arc::new(RuntimeHandle { _private: () }),
        })
    }

    /// Create a non-owning runtime that shares another runtime's handle.
    pub fn shared(handle: Arc<RuntimeHandle>) -> Self {
        Self { handle }
    }

    /// Return a shared handle to this runtime.
    pub fn handle(&self) -> Arc<RuntimeHandle> {
        Arc::clone(&self.handle)
    }

    /// Create a new unbound UDP socket.
    pub async fn create_socket(&self) -> Result<super::socket::UdxSocket> {
        Ok(super::socket::UdxSocket::new())
    }

    /// Create a new stream with the given local identifier.
    pub async fn create_stream(&self, local_id: u32) -> Result<super::stream::UdxStream> {
        tracing::debug!(local_id, "stream created");
        Ok(super::stream::UdxStream::new(local_id))
    }
}
