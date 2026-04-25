#![deny(clippy::all)]

mod common;

use common::proxy::{DirectionConfig, ProxyConfig};
use common::{create_lossy_pair, create_runtime, random_payload, verify_payload, with_timeout};
use std::time::{Duration, Instant};

const LONG_TIMEOUT: Duration = Duration::from_secs(300);

#[tokio::test]
async fn proxy_passthrough_no_impairments() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(Duration::from_secs(30), async {
        let runtime = create_runtime();
        let config = ProxyConfig::default();

        let (stream_a, mut stream_b, _sa, _sb, _proxy) =
            create_lossy_pair(&runtime, config).await;

        let total = 256 * 1024;
        let payload = random_payload(total);

        let payload_clone = payload.clone();
        let writer = tokio::spawn(async move {
            let chunk = 64 * 1024;
            let mut offset = 0;
            while offset < payload_clone.len() {
                let end = (offset + chunk).min(payload_clone.len());
                stream_a
                    .write(&payload_clone[offset..end])
                    .await
                    .expect("write");
                offset = end;
            }
        });

        let mut received = Vec::with_capacity(total);
        while received.len() < total {
            let chunk = stream_b.read().await.expect("read").expect("EOF");
            received.extend_from_slice(&chunk);
        }

        writer.await.expect("writer panicked");
        verify_payload(&payload, &received);
    })
    .await;
}

#[tokio::test]
async fn retransmission_under_loss() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(LONG_TIMEOUT, async {
        let runtime = create_runtime();
        // Loss A→B only: data packets drop, ACKs flow cleanly.
        let config = ProxyConfig {
            a_to_b: DirectionConfig {
                loss_rate: 0.05,
                ..Default::default()
            },
            b_to_a: DirectionConfig::default(),
        };

        let (stream_a, mut stream_b, _sa, _sb, _proxy) =
            create_lossy_pair(&runtime, config).await;

        let total = 256 * 1024;
        let payload = random_payload(total);

        let payload_clone = payload.clone();
        let writer = tokio::spawn(async move {
            let chunk = 64 * 1024;
            let mut offset = 0;
            while offset < payload_clone.len() {
                let end = (offset + chunk).min(payload_clone.len());
                stream_a
                    .write(&payload_clone[offset..end])
                    .await
                    .expect("write");
                offset = end;
            }
        });

        let mut received = Vec::with_capacity(total);
        while received.len() < total {
            let chunk = stream_b.read().await.expect("read").expect("EOF");
            received.extend_from_slice(&chunk);
        }

        writer.await.expect("writer panicked");
        verify_payload(&payload, &received);
    })
    .await;
}

#[tokio::test]
async fn rtt_under_delay() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(Duration::from_secs(30), async {
        let runtime = create_runtime();
        let config = ProxyConfig::symmetric(DirectionConfig {
            delay: Duration::from_millis(50),
            ..Default::default()
        });

        let (stream_a, mut stream_b, _sa, _sb, _proxy) =
            create_lossy_pair(&runtime, config).await;

        let start = Instant::now();
        stream_a.write(b"ping").await.expect("write");
        let elapsed = start.elapsed();

        let data = stream_b.read().await.expect("read").expect("EOF");
        assert_eq!(data, b"ping");

        // A→proxy(50ms)→B + B→proxy(50ms)→A ACK ≈ 100ms round-trip
        assert!(
            elapsed >= Duration::from_millis(80),
            "write completed too fast ({elapsed:?}), expected ≥80ms with 50ms delay each direction"
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "write took too long ({elapsed:?}), expected <5s"
        );
    })
    .await;
}

#[tokio::test]
async fn receive_window_respected() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(LONG_TIMEOUT, async {
        let runtime = create_runtime();
        // Proxy rewrites recv_window in B→A packets to 4096 bytes (~3 packets).
        // Sender A should respect this and limit inflight accordingly.
        let config = ProxyConfig {
            a_to_b: DirectionConfig::default(),
            b_to_a: DirectionConfig {
                recv_window_override: Some(4096),
                ..Default::default()
            },
        };

        let (stream_a, mut stream_b, _sa, _sb, _proxy) =
            create_lossy_pair(&runtime, config).await;

        let total = 512 * 1024;
        let payload = random_payload(total);

        let payload_clone = payload.clone();
        let writer = tokio::spawn(async move {
            let chunk = 64 * 1024;
            let mut offset = 0;
            while offset < payload_clone.len() {
                let end = (offset + chunk).min(payload_clone.len());
                stream_a
                    .write(&payload_clone[offset..end])
                    .await
                    .expect("write");
                offset = end;
            }
        });

        let mut received = Vec::with_capacity(total);
        while received.len() < total {
            let chunk = stream_b.read().await.expect("read").expect("EOF");
            received.extend_from_slice(&chunk);
        }

        writer.await.expect("writer panicked");
        verify_payload(&payload, &received);
    })
    .await;
}

#[tokio::test]
async fn congestion_recovery_after_burst_loss() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(LONG_TIMEOUT, async {
        let runtime = create_runtime();
        // 10% loss A→B only: high enough to trigger congestion recovery,
        // but ACKs flow cleanly so the sender can detect losses via SACK.
        let config = ProxyConfig {
            a_to_b: DirectionConfig {
                loss_rate: 0.10,
                ..Default::default()
            },
            b_to_a: DirectionConfig::default(),
        };

        let (stream_a, mut stream_b, _sa, _sb, _proxy) =
            create_lossy_pair(&runtime, config).await;

        let total = 256 * 1024;
        let payload = random_payload(total);

        let payload_clone = payload.clone();
        let writer = tokio::spawn(async move {
            let chunk = 64 * 1024;
            let mut offset = 0;
            while offset < payload_clone.len() {
                let end = (offset + chunk).min(payload_clone.len());
                stream_a
                    .write(&payload_clone[offset..end])
                    .await
                    .expect("write");
                offset = end;
            }
        });

        let mut received = Vec::with_capacity(total);
        while received.len() < total {
            let chunk = stream_b.read().await.expect("read").expect("EOF");
            received.extend_from_slice(&chunk);
        }

        writer.await.expect("writer panicked");
        verify_payload(&payload, &received);
    })
    .await;
}

#[tokio::test]
async fn reorder_handling() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(LONG_TIMEOUT, async {
        let runtime = create_runtime();
        let config = ProxyConfig::symmetric(DirectionConfig {
            reorder_rate: 0.10,
            ..Default::default()
        });

        let (stream_a, mut stream_b, _sa, _sb, _proxy) =
            create_lossy_pair(&runtime, config).await;

        let total = 1024 * 1024;
        let payload = random_payload(total);

        let payload_clone = payload.clone();
        let writer = tokio::spawn(async move {
            let chunk = 64 * 1024;
            let mut offset = 0;
            while offset < payload_clone.len() {
                let end = (offset + chunk).min(payload_clone.len());
                stream_a
                    .write(&payload_clone[offset..end])
                    .await
                    .expect("write");
                offset = end;
            }
        });

        let mut received = Vec::with_capacity(total);
        while received.len() < total {
            let chunk = stream_b.read().await.expect("read").expect("EOF");
            received.extend_from_slice(&chunk);
        }

        writer.await.expect("writer panicked");
        verify_payload(&payload, &received);
    })
    .await;
}

#[tokio::test]
async fn mtu_probing_discovers_path_mtu() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(LONG_TIMEOUT, async {
        let runtime = create_runtime();
        // Path MTU of 1400: probes step 1232→1264→…→1392 (pass), 1424 (dropped 3x) → MTU=1392
        let config = ProxyConfig::symmetric(DirectionConfig {
            max_packet_size: Some(1400),
            ..Default::default()
        });

        let (stream_a, mut stream_b, _sa, _sb, _proxy) =
            create_lossy_pair(&runtime, config).await;

        let total = 128 * 1024;
        let payload = random_payload(total);

        let payload_clone = payload.clone();
        let writer = tokio::spawn(async move {
            let chunk = 64 * 1024;
            let mut offset = 0;
            while offset < payload_clone.len() {
                let end = (offset + chunk).min(payload_clone.len());
                stream_a
                    .write(&payload_clone[offset..end])
                    .await
                    .expect("write");
                offset = end;
            }
            stream_a
        });

        let mut received = Vec::with_capacity(total);
        while received.len() < total {
            let chunk = stream_b.read().await.expect("read").expect("EOF");
            received.extend_from_slice(&chunk);
        }

        let stream_a = writer.await.expect("writer panicked");
        verify_payload(&payload, &received);

        let mtu = stream_a.effective_mtu();
        assert!(
            mtu > 1200 && mtu <= 1400,
            "effective_mtu should settle between 1200 and 1400, got {mtu}"
        );
    })
    .await;
}

#[tokio::test]
async fn mtu_probing_stays_at_base_when_clamped() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(LONG_TIMEOUT, async {
        let runtime = create_runtime();
        // Path MTU = MTU_BASE (1200): every probe (1232+) gets dropped → 3 failures → SearchComplete
        let config = ProxyConfig::symmetric(DirectionConfig {
            max_packet_size: Some(1200),
            ..Default::default()
        });

        let (stream_a, mut stream_b, _sa, _sb, _proxy) =
            create_lossy_pair(&runtime, config).await;

        let total = 64 * 1024;
        let payload = random_payload(total);

        let payload_clone = payload.clone();
        let writer = tokio::spawn(async move {
            let chunk = 32 * 1024;
            let mut offset = 0;
            while offset < payload_clone.len() {
                let end = (offset + chunk).min(payload_clone.len());
                stream_a
                    .write(&payload_clone[offset..end])
                    .await
                    .expect("write");
                offset = end;
            }
            stream_a
        });

        let mut received = Vec::with_capacity(total);
        while received.len() < total {
            let chunk = stream_b.read().await.expect("read").expect("EOF");
            received.extend_from_slice(&chunk);
        }

        let stream_a = writer.await.expect("writer panicked");
        verify_payload(&payload, &received);

        assert_eq!(
            stream_a.effective_mtu(),
            1200,
            "MTU should stay at base when path MTU = 1200"
        );
    })
    .await;
}

#[tokio::test]
async fn destroy_during_pending_write() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(Duration::from_secs(30), async {
        let runtime = create_runtime();

        let config = ProxyConfig::symmetric(DirectionConfig {
            delay: Duration::from_millis(200),
            ..Default::default()
        });

        let (stream_a, stream_b, _sa, _sb, _proxy) =
            create_lossy_pair(&runtime, config).await;

        let writer = tokio::spawn(async move {
            stream_a.write(b"hello from a").await
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        stream_b.destroy().await.expect("destroy");

        let result = tokio::time::timeout(
            Duration::from_secs(5),
            writer,
        )
        .await
        .expect("writer timed out — pending write was not drained")
        .expect("writer task panicked");

        assert!(
            result.is_err(),
            "write should fail when remote peer destroys during pending write"
        );
    })
    .await;
}
