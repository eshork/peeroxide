#![deny(clippy::all)]

//! Proves end-to-end encrypted communication through a UDX relay on the same
//! host. This is the exact mechanism used by Hyperswarm's blind-relay: the
//! relay forwards opaque UDP packets via `udx_stream_relay_to()` and never
//! sees plaintext.

use std::net::SocketAddr;

use libudx::UdxRuntime;
use peeroxide_dht::hyperdht_messages::{
    NoisePayload, SecretStreamInfo, UdxInfo, FIREWALL_UNKNOWN,
};
use peeroxide_dht::noise;
use peeroxide_dht::noise_wrap::NoiseWrap;
use peeroxide_dht::secret_stream::SecretStream;

const LOCALHOST: &str = "127.0.0.1:0";

fn addr() -> SocketAddr {
    LOCALHOST.parse().unwrap()
}

fn minimal_payload() -> NoisePayload {
    NoisePayload {
        version: 1,
        error: 0,
        firewall: FIREWALL_UNKNOWN,
        holepunch: None,
        addresses4: vec![],
        addresses6: vec![],
        udx: Some(UdxInfo {
            version: 1,
            reusable_socket: true,
            id: 0,
            seq: 0,
        }),
        secret_stream: Some(SecretStreamInfo { version: 1 }),
        relay_through: None,
        relay_addresses: None,
    }
}

/// Two peers do a Noise IK handshake in-memory, then communicate over
/// SecretStream through a UDX relay. The relay never sees plaintext.
#[tokio::test]
async fn encrypted_relay_same_host() {
    let _ = tracing_subscriber::fmt::try_init();

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        run_encrypted_relay_test(),
    )
    .await;

    match result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => panic!("encrypted relay test failed: {e}"),
        Err(_) => panic!("encrypted relay test timed out"),
    }
}

async fn run_encrypted_relay_test() -> Result<(), Box<dyn std::error::Error>> {
    let kp_a = noise::generate_keypair();
    let kp_b = noise::generate_keypair();

    let mut nw_a = NoiseWrap::new_initiator(
        noise::Keypair {
            public_key: kp_a.public_key,
            secret_key: kp_a.secret_key,
        },
        kp_b.public_key,
    );
    let mut nw_b = NoiseWrap::new_responder(noise::Keypair {
        public_key: kp_b.public_key,
        secret_key: kp_b.secret_key,
    });

    let m1 = nw_a.send(&minimal_payload())?;
    let _payload_from_a = nw_b.recv(&m1)?;
    let m2 = nw_b.send(&minimal_payload())?;
    let _payload_from_b = nw_a.recv(&m2)?;

    let result_a = nw_a.finalize()?;
    let result_b = nw_b.finalize()?;

    assert_eq!(result_a.remote_public_key, kp_b.public_key);
    assert_eq!(result_b.remote_public_key, kp_a.public_key);
    assert_eq!(result_a.handshake_hash, result_b.handshake_hash);

    let relay_rt = UdxRuntime::new()?;
    let peer_a_rt = UdxRuntime::new()?;
    let peer_b_rt = UdxRuntime::new()?;

    let relay_sock_1 = relay_rt.create_socket().await?;
    relay_sock_1.bind(addr()).await?;
    let relay_addr_1 = relay_sock_1.local_addr().await?;

    let relay_sock_2 = relay_rt.create_socket().await?;
    relay_sock_2.bind(addr()).await?;
    let relay_addr_2 = relay_sock_2.local_addr().await?;

    let relay_stream_1 = relay_rt.create_stream(10).await?;
    let relay_stream_2 = relay_rt.create_stream(20).await?;

    let peer_a_sock = peer_a_rt.create_socket().await?;
    peer_a_sock.bind(addr()).await?;
    let peer_a_addr = peer_a_sock.local_addr().await?;
    let peer_a_stream = peer_a_rt.create_stream(1).await?;

    let peer_b_sock = peer_b_rt.create_socket().await?;
    peer_b_sock.bind(addr()).await?;
    let peer_b_addr = peer_b_sock.local_addr().await?;
    let peer_b_stream = peer_b_rt.create_stream(2).await?;

    relay_stream_1.relay_to(&relay_stream_2)?;
    relay_stream_2.relay_to(&relay_stream_1)?;

    relay_stream_1
        .connect(&relay_sock_1, 1, peer_a_addr)
        .await?;
    relay_stream_2
        .connect(&relay_sock_2, 2, peer_b_addr)
        .await?;

    peer_a_stream
        .connect(&peer_a_sock, 10, relay_addr_1)
        .await?;
    peer_b_stream
        .connect(&peer_b_sock, 20, relay_addr_2)
        .await?;

    let peer_a_async = peer_a_stream.into_async_stream();
    let peer_b_async = peer_b_stream.into_async_stream();

    let (ss_a_result, ss_b_result) = tokio::join!(
        SecretStream::from_session(
            true,
            peer_a_async,
            result_a.tx,
            result_a.rx,
            result_a.handshake_hash,
            result_a.remote_public_key,
        ),
        SecretStream::from_session(
            false,
            peer_b_async,
            result_b.tx,
            result_b.rx,
            result_b.handshake_hash,
            result_b.remote_public_key,
        ),
    );

    let mut ss_a = ss_a_result?;
    let mut ss_b = ss_b_result?;

    ss_a.write(b"encrypted through relay A->B").await?;
    let msg = ss_b.read().await?.expect("should receive message");
    assert_eq!(msg, b"encrypted through relay A->B");

    ss_b.write(b"encrypted through relay B->A").await?;
    let msg = ss_a.read().await?.expect("should receive message");
    assert_eq!(msg, b"encrypted through relay B->A");

    for i in 0..10 {
        let payload = format!("message {i} from A");
        ss_a.write(payload.as_bytes()).await?;
        let msg = ss_b.read().await?.expect("should receive message");
        assert_eq!(msg, payload.as_bytes());
    }

    Ok(())
}
