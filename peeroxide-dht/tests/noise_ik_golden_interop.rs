use peeroxide_dht::noise::{keypair_from_seed, HandshakeIK};
use peeroxide_dht::noise_wrap::NoiseWrap;
use peeroxide_dht::crypto::{NS_PEER_HANDSHAKE, NS_PEER_HOLEPUNCH};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(tag = "type")]
#[allow(clippy::large_enum_variant)]
#[allow(clippy::enum_variant_names)]
enum Fixture {
    #[serde(rename = "noise_ik_handshake")]
    NoiseIk {
        label: String,
        static_initiator_seed: String,
        static_initiator_pk: String,
        static_responder_seed: String,
        static_responder_pk: String,
        ephemeral_initiator_seed: String,
        ephemeral_initiator_pk: String,
        ephemeral_responder_seed: String,
        ephemeral_responder_pk: String,
        prologue: String,
        #[serde(default)]
        payload_initiator: Option<String>,
        #[serde(default)]
        payload_responder: Option<String>,
        message1: String,
        message1_len: usize,
        message2: String,
        message2_len: usize,
        initiator_tx: String,
        initiator_rx: String,
        responder_tx: String,
        responder_rx: String,
        handshake_hash: String,
    },
    #[serde(rename = "noise_wrap")]
    NoiseWrapFixture {
        label: String,
        static_initiator_seed: String,
        static_initiator_pk: String,
        static_responder_seed: String,
        static_responder_pk: String,
        ephemeral_initiator_seed: String,
        ephemeral_initiator_pk: String,
        ephemeral_responder_seed: String,
        ephemeral_responder_pk: String,
        #[serde(rename = "payload_initiator_encoded")]
        _payload_initiator_encoded: String,
        #[serde(rename = "payload_responder_encoded")]
        _payload_responder_encoded: String,
        message1: String,
        message1_len: usize,
        message2: String,
        message2_len: usize,
        initiator_tx: String,
        initiator_rx: String,
        responder_tx: String,
        responder_rx: String,
        handshake_hash: String,
        holepunch_secret: String,
    },
    #[serde(rename = "holepunch_secret_derivation")]
    HolepunchSecret {
        label: String,
        handshake_hash: String,
        ns_peer_holepunch: String,
        holepunch_secret: String,
    },
}

fn h(hex_str: &str) -> Vec<u8> {
    hex::decode(hex_str).unwrap_or_else(|e| panic!("bad hex '{hex_str}': {e}"))
}

fn h32(hex_str: &str) -> [u8; 32] {
    let v = h(hex_str);
    v.try_into()
        .unwrap_or_else(|v: Vec<u8>| panic!("expected 32 bytes, got {}", v.len()))
}

fn h64(hex_str: &str) -> [u8; 64] {
    let v = h(hex_str);
    v.try_into()
        .unwrap_or_else(|v: Vec<u8>| panic!("expected 64 bytes, got {}", v.len()))
}

fn load_fixtures() -> Vec<Fixture> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tests/interop/noise-ik-fixtures.json"
    );
    let data = std::fs::read_to_string(path).unwrap_or_else(|e| {
        panic!(
            "Failed to read noise-ik fixtures at {path}: {e}. \
             Run `node generate-noise-ik-golden.js` in tests/node/ first."
        )
    });
    serde_json::from_str(&data).unwrap_or_else(|e| panic!("Failed to parse: {e}"))
}

#[test]
fn golden_noise_ik_handshake() {
    for fixture in load_fixtures() {
        let Fixture::NoiseIk {
            label,
            static_initiator_seed,
            static_initiator_pk,
            static_responder_seed,
            static_responder_pk,
            ephemeral_initiator_seed,
            ephemeral_initiator_pk,
            ephemeral_responder_seed,
            ephemeral_responder_pk,
            prologue,
            payload_initiator,
            payload_responder,
            message1,
            message1_len,
            message2,
            message2_len,
            initiator_tx,
            initiator_rx,
            responder_tx,
            responder_rx,
            handshake_hash,
        } = fixture
        else {
            continue;
        };

        let prologue_bytes = h(&prologue);
        assert_eq!(
            prologue_bytes,
            &*NS_PEER_HANDSHAKE,
            "{label}: prologue should be NS_PEER_HANDSHAKE"
        );

        let s_i = keypair_from_seed(&h32(&static_initiator_seed));
        assert_eq!(hex::encode(s_i.public_key), static_initiator_pk, "{label}: static_i pk");

        let s_r = keypair_from_seed(&h32(&static_responder_seed));
        assert_eq!(hex::encode(s_r.public_key), static_responder_pk, "{label}: static_r pk");

        let e_i = keypair_from_seed(&h32(&ephemeral_initiator_seed));
        assert_eq!(hex::encode(e_i.public_key), ephemeral_initiator_pk, "{label}: eph_i pk");

        let e_r = keypair_from_seed(&h32(&ephemeral_responder_seed));
        assert_eq!(hex::encode(e_r.public_key), ephemeral_responder_pk, "{label}: eph_r pk");

        let payload_i = payload_initiator.as_deref().map(h).unwrap_or_default();
        let payload_r = payload_responder.as_deref().map(h).unwrap_or_default();

        let mut init = HandshakeIK::new_initiator(s_i, s_r.public_key, &prologue_bytes);
        init.set_ephemeral(e_i);
        let mut resp = HandshakeIK::new_responder(s_r, &prologue_bytes);

        let m1 = init.send(&payload_i).expect("send M1");
        assert_eq!(m1.len(), message1_len, "{label}: M1 length");
        assert_eq!(hex::encode(&m1), message1, "{label}: M1 bytes");

        let recv_pi = resp.recv(&m1).expect("recv M1");
        assert_eq!(recv_pi, payload_i, "{label}: M1 payload mismatch");

        resp.set_ephemeral(e_r);
        let m2 = resp.send(&payload_r).expect("send M2");
        assert_eq!(m2.len(), message2_len, "{label}: M2 length");
        assert_eq!(hex::encode(&m2), message2, "{label}: M2 bytes");

        let recv_pr = init.recv(&m2).expect("recv M2");
        assert_eq!(recv_pr, payload_r, "{label}: M2 payload mismatch");

        assert!(init.complete(), "{label}: initiator not complete");
        assert!(resp.complete(), "{label}: responder not complete");

        let ir = init.result().expect("initiator result");
        let rr = resp.result().expect("responder result");

        assert_eq!(hex::encode(ir.tx), initiator_tx, "{label}: initiator tx");
        assert_eq!(hex::encode(ir.rx), initiator_rx, "{label}: initiator rx");
        assert_eq!(hex::encode(rr.tx), responder_tx, "{label}: responder tx");
        assert_eq!(hex::encode(rr.rx), responder_rx, "{label}: responder rx");
        assert_eq!(hex::encode(ir.handshake_hash), handshake_hash, "{label}: hash");
        assert_eq!(ir.handshake_hash, rr.handshake_hash, "{label}: hash agreement");
    }
}

#[test]
fn golden_noise_wrap_handshake() {
    use peeroxide_dht::hyperdht_messages::NoisePayload;

    for fixture in load_fixtures() {
        let Fixture::NoiseWrapFixture {
            label,
            static_initiator_seed,
            static_initiator_pk,
            static_responder_seed,
            static_responder_pk,
            ephemeral_initiator_seed,
            ephemeral_initiator_pk,
            ephemeral_responder_seed,
            ephemeral_responder_pk,
            _payload_initiator_encoded: _,
            _payload_responder_encoded: _,
            message1,
            message1_len,
            message2,
            message2_len,
            initiator_tx,
            initiator_rx,
            responder_tx,
            responder_rx,
            handshake_hash,
            holepunch_secret,
        } = fixture
        else {
            continue;
        };

        let s_i = keypair_from_seed(&h32(&static_initiator_seed));
        assert_eq!(hex::encode(s_i.public_key), static_initiator_pk, "{label}: pk_i");

        let s_r = keypair_from_seed(&h32(&static_responder_seed));
        assert_eq!(hex::encode(s_r.public_key), static_responder_pk, "{label}: pk_r");

        let e_i = keypair_from_seed(&h32(&ephemeral_initiator_seed));
        assert_eq!(hex::encode(e_i.public_key), ephemeral_initiator_pk, "{label}: eph_i");

        let e_r = keypair_from_seed(&h32(&ephemeral_responder_seed));
        assert_eq!(hex::encode(e_r.public_key), ephemeral_responder_pk, "{label}: eph_r");

        let payload_i = NoisePayload {
            version: 1,
            error: 0,
            firewall: 2,
            holepunch: None,
            addresses4: vec![peeroxide_dht::messages::Ipv4Peer {
                host: "192.168.1.10".to_string(),
                port: 9000,
            }],
            addresses6: vec![],
            udx: Some(peeroxide_dht::hyperdht_messages::UdxInfo {
                version: 1,
                reusable_socket: false,
                id: 1,
                seq: 0,
            }),
            secret_stream: Some(peeroxide_dht::hyperdht_messages::SecretStreamInfo { version: 1 }),
            relay_through: None,
            relay_addresses: None,
        };

        let payload_r = NoisePayload {
            version: 1,
            error: 0,
            firewall: 1,
            holepunch: Some(peeroxide_dht::hyperdht_messages::HolepunchInfo {
                id: 42,
                relays: vec![],
            }),
            addresses4: vec![peeroxide_dht::messages::Ipv4Peer {
                host: "10.0.0.1".to_string(),
                port: 8080,
            }],
            addresses6: vec![],
            udx: Some(peeroxide_dht::hyperdht_messages::UdxInfo {
                version: 1,
                reusable_socket: true,
                id: 7,
                seq: 3,
            }),
            secret_stream: Some(peeroxide_dht::hyperdht_messages::SecretStreamInfo { version: 1 }),
            relay_through: None,
            relay_addresses: None,
        };

        let mut init = NoiseWrap::new_initiator(s_i, s_r.public_key);
        init.set_ephemeral(e_i);
        let mut resp = NoiseWrap::new_responder(s_r);

        let m1 = init.send(&payload_i).expect("send M1");
        assert_eq!(m1.len(), message1_len, "{label}: M1 length");
        assert_eq!(hex::encode(&m1), message1, "{label}: M1 bytes");

        let recv_pi = resp.recv(&m1).expect("recv M1");
        assert_eq!(recv_pi.firewall, 2, "{label}: M1 payload firewall");
        assert_eq!(recv_pi.addresses4.len(), 1, "{label}: M1 payload addresses4 len");
        assert_eq!(recv_pi.addresses4[0].port, 9000, "{label}: M1 payload port");

        resp.set_ephemeral(e_r);
        let m2 = resp.send(&payload_r).expect("send M2");
        assert_eq!(m2.len(), message2_len, "{label}: M2 length");
        assert_eq!(hex::encode(&m2), message2, "{label}: M2 bytes");

        let recv_pr = init.recv(&m2).expect("recv M2");
        assert_eq!(recv_pr.firewall, 1, "{label}: M2 payload firewall");
        assert_eq!(recv_pr.holepunch.as_ref().unwrap().id, 42, "{label}: M2 holepunch id");
        assert_eq!(recv_pr.udx.as_ref().unwrap().id, 7, "{label}: M2 udx id");

        let result_i = init.finalize().expect("finalize initiator");
        let result_r = resp.finalize().expect("finalize responder");

        assert_eq!(hex::encode(result_i.tx), initiator_tx, "{label}: init tx");
        assert_eq!(hex::encode(result_i.rx), initiator_rx, "{label}: init rx");
        assert_eq!(hex::encode(result_r.tx), responder_tx, "{label}: resp tx");
        assert_eq!(hex::encode(result_r.rx), responder_rx, "{label}: resp rx");
        assert_eq!(hex::encode(result_i.handshake_hash), handshake_hash, "{label}: hash");
        assert_eq!(
            hex::encode(result_i.holepunch_secret),
            holepunch_secret,
            "{label}: holepunch secret"
        );
        assert_eq!(
            result_i.holepunch_secret, result_r.holepunch_secret,
            "{label}: holepunch secret agreement"
        );
    }
}

#[test]
fn golden_holepunch_secret_derivation() {
    use blake2::digest::consts::U32;
    use blake2::digest::{KeyInit, Mac};
    use blake2::Blake2bMac;

    type Blake2bMac256 = Blake2bMac<U32>;

    for fixture in load_fixtures() {
        let Fixture::HolepunchSecret {
            label,
            handshake_hash,
            ns_peer_holepunch,
            holepunch_secret,
        } = fixture
        else {
            continue;
        };

        assert_eq!(
            h(&ns_peer_holepunch),
            &*NS_PEER_HOLEPUNCH,
            "{label}: NS_PEER_HOLEPUNCH mismatch"
        );

        let hash = h64(&handshake_hash);

        let mut mac: Blake2bMac256 =
            KeyInit::new_from_slice(&hash[..]).expect("64-byte key");
        Mac::update(&mut mac, &*NS_PEER_HOLEPUNCH);
        let computed = mac.finalize().into_bytes();
        let mut computed_arr = [0u8; 32];
        computed_arr.copy_from_slice(&computed);

        assert_eq!(
            hex::encode(computed_arr),
            holepunch_secret,
            "{label}: holepunch secret mismatch"
        );
    }
}
