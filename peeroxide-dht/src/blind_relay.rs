//! blind-relay protocol messages — wire-compatible with Node.js `blind-relay@1.4.0`.
//!
//! The blind-relay protocol uses Protomux with protocol name `"blind-relay"`.
//! It has exactly two message types:
//!
//! - **Pair** (type 0): Request to pair two connections through the relay.
//! - **Unpair** (type 1): Cancel a previous pair request.
//!
//! # Wire Format
//!
//! ## Pair (message type 0)
//! ```text
//! [bitfield(7): flags, bit0=isInitiator] [fixed32: token] [uint: id] [uint: seq]
//! ```
//!
//! ## Unpair (message type 1)
//! ```text
//! [bitfield(7): flags, all zero] [fixed32: token]
//! ```
//!
//! The `bitfield(7)` from `compact-encoding-bitfield` is a single byte holding
//! up to 7 boolean flags. Only bit 0 (`is_initiator`) is used.

use crate::compact_encoding::{self as c, State};
use crate::protomux::{self, Channel, ChannelEvent, Mux};
use thiserror::Error;
use tracing::debug;

/// Pair message — requests relay pairing with a 32-byte token.
#[derive(Debug, Clone, PartialEq)]
pub struct PairMessage {
    pub is_initiator: bool,
    pub token: [u8; 32],
    pub id: u64,
    pub seq: u64,
}

/// Unpair message — cancels a relay pairing.
#[derive(Debug, Clone, PartialEq)]
pub struct UnpairMessage {
    pub token: [u8; 32],
}

/// Protocol name used over Protomux.
pub const PROTOCOL_NAME: &str = "blind-relay";

/// Protomux message type index for pair.
pub const MSG_TYPE_PAIR: u32 = 0;

/// Protomux message type index for unpair.
pub const MSG_TYPE_UNPAIR: u32 = 1;

pub fn preencode_pair(state: &mut State, msg: &PairMessage) {
    state.end += 1; // bitfield(7) = 1 byte
    state.end += 32; // fixed32 token
    c::preencode_uint(state, msg.id);
    c::preencode_uint(state, msg.seq);
}

pub fn encode_pair(state: &mut State, msg: &PairMessage) {
    let flags: u8 = if msg.is_initiator { 1 } else { 0 };
    c::encode_uint8(state, flags);
    c::encode_fixed32(state, &msg.token);
    c::encode_uint(state, msg.id);
    c::encode_uint(state, msg.seq);
}

pub fn decode_pair(state: &mut State) -> c::Result<PairMessage> {
    let flags = c::decode_uint8(state)?;
    let is_initiator = flags & 1 != 0;
    let token = c::decode_fixed32(state)?;
    let id = c::decode_uint(state)?;
    let seq = c::decode_uint(state)?;
    Ok(PairMessage {
        is_initiator,
        token,
        id,
        seq,
    })
}

pub fn preencode_unpair(state: &mut State, _msg: &UnpairMessage) {
    state.end += 1; // bitfield(7) = 1 byte
    state.end += 32; // fixed32 token
}

pub fn encode_unpair(state: &mut State, msg: &UnpairMessage) {
    c::encode_uint8(state, 0); // flags = 0
    c::encode_fixed32(state, &msg.token);
}

pub fn decode_unpair(state: &mut State) -> c::Result<UnpairMessage> {
    let _flags = c::decode_uint8(state)?;
    let token = c::decode_fixed32(state)?;
    Ok(UnpairMessage { token })
}

/// Encode a pair message to bytes (preencode + allocate + encode).
pub fn encode_pair_to_vec(msg: &PairMessage) -> Vec<u8> {
    let mut state = State::new();
    preencode_pair(&mut state, msg);
    state.alloc();
    encode_pair(&mut state, msg);
    state.buffer
}

/// Encode an unpair message to bytes.
pub fn encode_unpair_to_vec(msg: &UnpairMessage) -> Vec<u8> {
    let mut state = State::new();
    preencode_unpair(&mut state, msg);
    state.alloc();
    encode_unpair(&mut state, msg);
    state.buffer
}

/// Decode a pair message from bytes.
pub fn decode_pair_from_slice(data: &[u8]) -> c::Result<PairMessage> {
    let mut state = State::from_buffer(data);
    decode_pair(&mut state)
}

/// Decode an unpair message from bytes.
pub fn decode_unpair_from_slice(data: &[u8]) -> c::Result<UnpairMessage> {
    let mut state = State::from_buffer(data);
    decode_unpair(&mut state)
}

// ── Client ───────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum RelayError {
    #[error("protomux error: {0}")]
    Protomux(#[from] protomux::ProtomuxError),

    #[error("encoding error: {0}")]
    Encoding(#[from] c::EncodingError),

    #[error("channel closed before pair response")]
    ChannelClosed,

    #[error("relay client destroyed")]
    Destroyed,

    #[error("already pairing with this token")]
    AlreadyPairing,
}

/// Response from a successful relay pairing.
#[derive(Debug, Clone)]
pub struct PairResponse {
    pub remote_id: u64,
}

/// Client-side blind-relay over an existing Protomux connection.
///
/// Wraps a Protomux channel with protocol `"blind-relay"`. Sends pair/unpair
/// messages and waits for the relay server to match the token.
pub struct BlindRelayClient {
    channel: Channel,
}

impl BlindRelayClient {
    /// Open a blind-relay channel on the given Mux.
    ///
    /// `id` should be the local public key used when connecting to the relay.
    /// Both the relay server and the connecting peer must use the same `id`
    /// (the connecting peer's public key) so that Protomux can pair the
    /// channels correctly.
    ///
    /// Sends the Open frame immediately. Call [`Self::wait_opened`] before
    /// sending pair/unpair messages.
    pub async fn open(mux: &Mux, id: Option<Vec<u8>>) -> Result<Self, RelayError> {
        let channel = mux.create_channel(PROTOCOL_NAME, id, None).await?;
        Ok(Self { channel })
    }

    /// Wait for the remote side to open the channel.
    pub async fn wait_opened(&mut self) -> Result<(), RelayError> {
        self.channel.wait_opened().await?;
        Ok(())
    }

    /// Send a pair request and wait for the relay server's response.
    ///
    /// Returns the relay-assigned `remote_id` (UDX stream ID on the relay side).
    /// Blocks until the server sends a matching pair message back.
    pub async fn pair(
        &mut self,
        is_initiator: bool,
        token: &[u8; 32],
        stream_id: u64,
    ) -> Result<PairResponse, RelayError> {
        let msg = PairMessage {
            is_initiator,
            token: *token,
            id: stream_id,
            seq: 0,
        };
        self.channel
            .send(MSG_TYPE_PAIR, &encode_pair_to_vec(&msg))?;

        debug!(
            is_initiator,
            token = %format_args!("{:02x?}", &token[..4]),
            stream_id,
            "sent pair request"
        );

        loop {
            match self.channel.recv().await {
                Some(ChannelEvent::Message { message_type, data }) => {
                    if message_type == MSG_TYPE_PAIR {
                        let response = decode_pair_from_slice(&data)?;
                        if response.token == *token && response.is_initiator == is_initiator {
                            debug!(
                                remote_id = response.id,
                                "pair response received"
                            );
                            return Ok(PairResponse {
                                remote_id: response.id,
                            });
                        }
                    }
                }
                Some(ChannelEvent::Closed { .. }) | None => {
                    return Err(RelayError::ChannelClosed);
                }
                Some(ChannelEvent::Opened { .. }) => {}
            }
        }
    }

    /// Cancel a pending pair request.
    pub fn unpair(&self, token: &[u8; 32]) -> Result<(), RelayError> {
        let msg = UnpairMessage { token: *token };
        self.channel
            .send(MSG_TYPE_UNPAIR, &encode_unpair_to_vec(&msg))?;
        Ok(())
    }

    /// Close the blind-relay channel.
    pub fn close(&mut self) {
        self.channel.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protomux::{FramedStream, Mux};
    use tokio::sync::mpsc;

    struct MemStream {
        rx: mpsc::UnboundedReceiver<Vec<u8>>,
        tx: mpsc::UnboundedSender<Vec<u8>>,
    }

    impl FramedStream for MemStream {
        async fn read_frame(&mut self) -> std::io::Result<Option<Vec<u8>>> {
            Ok(self.rx.recv().await)
        }

        async fn write_frame(&mut self, data: &[u8]) -> std::io::Result<()> {
            self.tx
                .send(data.to_vec())
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "closed"))
        }
    }

    fn mem_pair() -> (MemStream, MemStream) {
        let (tx_a, rx_b) = mpsc::unbounded_channel();
        let (tx_b, rx_a) = mpsc::unbounded_channel();
        (
            MemStream { rx: rx_a, tx: tx_a },
            MemStream { rx: rx_b, tx: tx_b },
        )
    }

    #[test]
    fn pair_roundtrip_initiator() {
        let msg = PairMessage {
            is_initiator: true,
            token: [0xaa; 32],
            id: 42,
            seq: 7,
        };
        let encoded = encode_pair_to_vec(&msg);
        let decoded = decode_pair_from_slice(&encoded).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn pair_roundtrip_responder() {
        let msg = PairMessage {
            is_initiator: false,
            token: [0xbb; 32],
            id: 0,
            seq: 0,
        };
        let encoded = encode_pair_to_vec(&msg);
        let decoded = decode_pair_from_slice(&encoded).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn unpair_roundtrip() {
        let msg = UnpairMessage {
            token: [0xcc; 32],
        };
        let encoded = encode_unpair_to_vec(&msg);
        let decoded = decode_unpair_from_slice(&encoded).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn pair_wire_format() {
        let msg = PairMessage {
            is_initiator: true,
            token: [0x42; 32],
            id: 1,
            seq: 2,
        };
        let encoded = encode_pair_to_vec(&msg);

        assert_eq!(encoded[0], 0x01); // flags: bit0=1 (initiator)
        assert_eq!(&encoded[1..33], &[0x42; 32]); // token
        assert_eq!(encoded[33], 0x01); // id=1 (varint)
        assert_eq!(encoded[34], 0x02); // seq=2 (varint)
        assert_eq!(encoded.len(), 35);
    }

    #[test]
    fn pair_wire_format_responder() {
        let msg = PairMessage {
            is_initiator: false,
            token: [0x00; 32],
            id: 0,
            seq: 0,
        };
        let encoded = encode_pair_to_vec(&msg);

        assert_eq!(encoded[0], 0x00); // flags: bit0=0 (responder)
        assert_eq!(&encoded[1..33], &[0x00; 32]); // token
        assert_eq!(encoded[33], 0x00); // id=0
        assert_eq!(encoded[34], 0x00); // seq=0
    }

    #[test]
    fn unpair_wire_format() {
        let msg = UnpairMessage {
            token: [0xff; 32],
        };
        let encoded = encode_unpair_to_vec(&msg);

        assert_eq!(encoded[0], 0x00); // flags: all zero
        assert_eq!(&encoded[1..33], &[0xff; 32]); // token
        assert_eq!(encoded.len(), 33);
    }

    #[test]
    fn pair_large_ids() {
        let msg = PairMessage {
            is_initiator: true,
            token: [0xde; 32],
            id: 100_000,
            seq: 65_536,
        };
        let encoded = encode_pair_to_vec(&msg);
        let decoded = decode_pair_from_slice(&encoded).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn protocol_name_constant() {
        assert_eq!(PROTOCOL_NAME, "blind-relay");
    }

    #[tokio::test]
    async fn client_pair_with_fake_relay() {
        let (stream_a, stream_b) = mem_pair();

        let (mux_a, run_a) = Mux::new(stream_a);
        let (mux_b, run_b) = Mux::new(stream_b);

        tokio::spawn(run_a);
        tokio::spawn(run_b);

        let token = [0xaa; 32];

        let client_task = tokio::spawn(async move {
            let mut client = BlindRelayClient::open(&mux_a, None).await.unwrap();
            client.wait_opened().await.unwrap();
            let resp = client.pair(true, &token, 42).await.unwrap();
            client.close();
            resp
        });

        // Fake relay server: open matching channel, wait for pair, send response
        let mut server_ch = mux_b
            .create_channel(PROTOCOL_NAME, None, None)
            .await
            .unwrap();
        server_ch.wait_opened().await.unwrap();

        let event = server_ch.recv().await.unwrap();
        match event {
            ChannelEvent::Message { message_type, data } => {
                assert_eq!(message_type, MSG_TYPE_PAIR);
                let pair_msg = decode_pair_from_slice(&data).unwrap();
                assert!(pair_msg.is_initiator);
                assert_eq!(pair_msg.token, token);
                assert_eq!(pair_msg.id, 42);

                let reply = PairMessage {
                    is_initiator: true,
                    token,
                    id: 99,
                    seq: 0,
                };
                server_ch
                    .send(MSG_TYPE_PAIR, &encode_pair_to_vec(&reply))
                    .unwrap();
            }
            other => panic!("expected pair Message, got {other:?}"),
        }

        let resp = client_task.await.unwrap();
        assert_eq!(resp.remote_id, 99);
    }

    #[tokio::test]
    async fn client_unpair() {
        let (stream_a, stream_b) = mem_pair();

        let (mux_a, run_a) = Mux::new(stream_a);
        let (mux_b, run_b) = Mux::new(stream_b);

        tokio::spawn(run_a);
        tokio::spawn(run_b);

        let token = [0xbb; 32];

        let mut client = BlindRelayClient::open(&mux_a, None).await.unwrap();
        let mut server_ch = mux_b
            .create_channel(PROTOCOL_NAME, None, None)
            .await
            .unwrap();

        client.wait_opened().await.unwrap();
        server_ch.wait_opened().await.unwrap();

        client.unpair(&token).unwrap();

        let event = server_ch.recv().await.unwrap();
        match event {
            ChannelEvent::Message { message_type, data } => {
                assert_eq!(message_type, MSG_TYPE_UNPAIR);
                let unpair_msg = decode_unpair_from_slice(&data).unwrap();
                assert_eq!(unpair_msg.token, token);
            }
            other => panic!("expected unpair Message, got {other:?}"),
        }

        client.close();
    }
}
