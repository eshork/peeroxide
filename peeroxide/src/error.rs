use thiserror::Error;

/// Errors from the Hyperswarm layer.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SwarmError {
    /// Error from the HyperDHT layer.
    #[error("DHT error: {0}")]
    Dht(#[from] peeroxide_dht::hyperdht::HyperDhtError),

    /// Error from the DHT RPC transport.
    #[error("DHT RPC error: {0}")]
    DhtRpc(#[from] peeroxide_dht::rpc::DhtError),

    /// Error from the UDX transport layer.
    #[error("UDX error: {0}")]
    Udx(#[from] libudx::UdxError),

    /// The swarm has been destroyed and can no longer process commands.
    #[error("swarm destroyed")]
    Destroyed,

    /// An internal channel was closed unexpectedly.
    #[error("channel closed")]
    ChannelClosed,
}
