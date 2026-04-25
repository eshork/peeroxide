use thiserror::Error;

/// Errors from the Hyperswarm layer.
#[derive(Debug, Error)]
pub enum SwarmError {
    #[error("DHT error: {0}")]
    Dht(#[from] peeroxide_dht::hyperdht::HyperDhtError),

    #[error("DHT RPC error: {0}")]
    DhtRpc(#[from] peeroxide_dht::rpc::DhtError),

    #[error("UDX error: {0}")]
    Udx(#[from] libudx::UdxError),

    #[error("swarm destroyed")]
    Destroyed,

    #[error("channel closed")]
    ChannelClosed,
}
