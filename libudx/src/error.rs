/// Errors returned by libudx operations.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum UdxError {
    /// A libuv/udx numeric error code. Retained for API compatibility with
    /// the former FFI backend; the native implementation returns [`Io`](Self::Io)
    /// or [`StreamClosed`](Self::StreamClosed) instead.
    #[error("libuv/udx error code: {0}")]
    Uv(i32),
    /// The UDX runtime has shut down and can no longer process requests.
    #[error("runtime shut down")]
    RuntimeGone,
    /// The stream has been closed or destroyed.
    #[error("stream closed")]
    StreamClosed,
    /// An underlying I/O error from the operating system or tokio.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Convenience alias for `std::result::Result<T, UdxError>`.
pub type Result<T> = std::result::Result<T, UdxError>;
