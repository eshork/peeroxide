use peeroxide_dht::noise::{ed25519_dh, keypair_from_seed, Handshake, Keypair};
use peeroxide_dht::secretstream::{Pull, Push, ABYTES, HEADERBYTES, KEYBYTES, TAG_FINAL, TAG_MESSAGE};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(tag = "type")]
#[allow(clippy::large_enum_variant)]
enum Fixture {
    #[serde(rename = "ed25519_dh")]
    Ed25519Dh {
        label: String,
        seed_a: String,
        public_key_a: String,
        secret_key_a: String,
        seed_b: String,
        public_key_b: String,
        secret_key_b: String,
        dh_output: String,
    },
    #[serde(rename = "noise_xx_handshake")]
    NoiseXx {
        label: String,
        static_initiator_seed: String,
        static_initiator_pk: String,
        static_responder_seed: String,
        static_responder_pk: String,
        ephemeral_initiator_seed: String,
        ephemeral_initiator_pk: String,
        ephemeral_responder_seed: String,
        ephemeral_responder_pk: String,
        message1: String,
        message1_len: usize,
        message2: String,
        message2_len: usize,
        message3: String,
        message3_len: usize,
        initiator_tx: String,
        initiator_rx: String,
        responder_tx: String,
        responder_rx: String,
        handshake_hash: String,
    },
    #[serde(rename = "secretstream")]
    Secretstream {
        label: String,
        key: String,
        header: String,
        messages: Vec<SsMessage>,
        abytes: usize,
        headerbytes: usize,
        keybytes: usize,
    },
}

#[derive(Deserialize)]
struct SsMessage {
    plaintext: String,
    ciphertext: String,
    tag: String,
}

fn h(hex_str: &str) -> Vec<u8> {
    hex::decode(hex_str).unwrap_or_else(|e| panic!("bad hex '{hex_str}': {e}"))
}

fn h32(hex_str: &str) -> [u8; 32] {
    let v = h(hex_str);
    v.try_into().unwrap_or_else(|v: Vec<u8>| panic!("expected 32 bytes, got {}", v.len()))
}

fn load_fixtures() -> Vec<Fixture> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tests/interop/noise-fixtures.json"
    );
    let data = std::fs::read_to_string(path).unwrap_or_else(|e| {
        panic!(
            "Failed to read noise fixtures at {path}: {e}. \
             Run `node generate-noise-golden.js` in tests/node/ first."
        )
    });
    serde_json::from_str(&data).unwrap_or_else(|e| panic!("Failed to parse noise fixtures: {e}"))
}

fn kp_from_seed(seed_hex: &str) -> Keypair {
    keypair_from_seed(&h32(seed_hex))
}

// ── Ed25519 DH ──────────────────────────────────────────────────────────────

#[test]
fn golden_ed25519_dh() {
    for fixture in load_fixtures() {
        let Fixture::Ed25519Dh {
            label,
            seed_a,
            public_key_a,
            secret_key_a,
            seed_b,
            public_key_b,
            secret_key_b,
            dh_output,
        } = fixture
        else {
            continue;
        };

        let kp_a = kp_from_seed(&seed_a);
        assert_eq!(hex::encode(kp_a.public_key), public_key_a, "{label}: pk_a mismatch");
        assert_eq!(hex::encode(kp_a.secret_key), secret_key_a, "{label}: sk_a mismatch");

        let kp_b = kp_from_seed(&seed_b);
        assert_eq!(hex::encode(kp_b.public_key), public_key_b, "{label}: pk_b mismatch");
        assert_eq!(hex::encode(kp_b.secret_key), secret_key_b, "{label}: sk_b mismatch");

        let dh_ab = ed25519_dh(&kp_a.secret_key, &kp_b.public_key).expect("dh(a,b)");
        assert_eq!(hex::encode(dh_ab), dh_output, "{label}: dh(a,b) mismatch");

        let dh_ba = ed25519_dh(&kp_b.secret_key, &kp_a.public_key).expect("dh(b,a)");
        assert_eq!(hex::encode(dh_ba), dh_output, "{label}: dh(b,a) != dh(a,b)");
    }
}

// ── Noise XX Handshake ──────────────────────────────────────────────────────

#[test]
fn golden_noise_xx_handshake() {
    for fixture in load_fixtures() {
        let Fixture::NoiseXx {
            label,
            static_initiator_seed,
            static_initiator_pk,
            static_responder_seed,
            static_responder_pk,
            ephemeral_initiator_seed,
            ephemeral_initiator_pk,
            ephemeral_responder_seed,
            ephemeral_responder_pk,
            message1,
            message1_len,
            message2,
            message2_len,
            message3,
            message3_len,
            initiator_tx,
            initiator_rx,
            responder_tx,
            responder_rx,
            handshake_hash,
        } = fixture
        else {
            continue;
        };

        let s_i = kp_from_seed(&static_initiator_seed);
        assert_eq!(hex::encode(s_i.public_key), static_initiator_pk, "{label}: static_i pk");

        let s_r = kp_from_seed(&static_responder_seed);
        assert_eq!(hex::encode(s_r.public_key), static_responder_pk, "{label}: static_r pk");

        let e_i = kp_from_seed(&ephemeral_initiator_seed);
        assert_eq!(hex::encode(e_i.public_key), ephemeral_initiator_pk, "{label}: eph_i pk");

        let e_r = kp_from_seed(&ephemeral_responder_seed);
        assert_eq!(hex::encode(e_r.public_key), ephemeral_responder_pk, "{label}: eph_r pk");

        let mut initiator = Handshake::new(true, s_i);
        initiator.set_ephemeral(e_i);
        let mut responder = Handshake::new(false, s_r);

        // M1: initiator → responder
        let m1 = initiator.send().expect("send M1");
        assert_eq!(m1.len(), message1_len, "{label}: M1 length");
        assert_eq!(hex::encode(&m1), message1, "{label}: M1 bytes");

        responder.recv(&m1).expect("recv M1");

        // M2: responder → initiator (inject ephemeral before send)
        responder.set_ephemeral(e_r);
        let m2 = responder.send().expect("send M2");
        assert_eq!(m2.len(), message2_len, "{label}: M2 length");
        assert_eq!(hex::encode(&m2), message2, "{label}: M2 bytes");

        initiator.recv(&m2).expect("recv M2");

        // M3: initiator → responder
        let m3 = initiator.send().expect("send M3");
        assert_eq!(m3.len(), message3_len, "{label}: M3 length");
        assert_eq!(hex::encode(&m3), message3, "{label}: M3 bytes");

        assert!(initiator.complete(), "{label}: initiator should be complete");
        let ir = initiator.result().expect("initiator result");

        let rr = responder.recv(&m3).expect("recv M3").expect("responder result");
        assert!(responder.complete(), "{label}: responder should be complete");

        assert_eq!(hex::encode(ir.tx), initiator_tx, "{label}: initiator tx");
        assert_eq!(hex::encode(ir.rx), initiator_rx, "{label}: initiator rx");
        assert_eq!(hex::encode(rr.tx), responder_tx, "{label}: responder tx");
        assert_eq!(hex::encode(rr.rx), responder_rx, "{label}: responder rx");
        assert_eq!(hex::encode(ir.handshake_hash), handshake_hash, "{label}: handshake hash");
        assert_eq!(ir.handshake_hash, rr.handshake_hash, "{label}: hash agreement");
    }
}

// ── Secretstream ────────────────────────────────────────────────────────────

#[test]
fn golden_secretstream_constants() {
    for fixture in load_fixtures() {
        let Fixture::Secretstream {
            abytes,
            headerbytes,
            keybytes,
            ..
        } = fixture
        else {
            continue;
        };
        assert_eq!(abytes, ABYTES, "ABYTES mismatch");
        assert_eq!(headerbytes, HEADERBYTES, "HEADERBYTES mismatch");
        assert_eq!(keybytes, KEYBYTES, "KEYBYTES mismatch");
    }
}

#[test]
fn golden_secretstream_decrypt() {
    for fixture in load_fixtures() {
        let Fixture::Secretstream {
            label,
            key,
            header,
            messages,
            ..
        } = fixture
        else {
            continue;
        };

        let key_bytes = h32(&key);
        let header_bytes: [u8; HEADERBYTES] = h(&header)
            .try_into()
            .unwrap_or_else(|v: Vec<u8>| panic!("{label}: header len {}", v.len()));

        let mut pull = Pull::new(&key_bytes, &header_bytes);

        for (i, msg) in messages.iter().enumerate() {
            let ct = h(&msg.ciphertext);
            let (plaintext, tag) = pull.next(&ct).unwrap_or_else(|e| {
                panic!("{label} msg[{i}]: decrypt failed: {e}")
            });

            let expected_pt = h(&msg.plaintext);
            assert_eq!(plaintext, expected_pt, "{label} msg[{i}]: plaintext mismatch");

            let expected_tag = match msg.tag.as_str() {
                "message" => TAG_MESSAGE,
                "final" => TAG_FINAL,
                other => panic!("{label} msg[{i}]: unknown tag '{other}'"),
            };
            assert_eq!(tag, expected_tag, "{label} msg[{i}]: tag mismatch");
        }
    }
}

#[test]
fn golden_secretstream_encrypt() {
    for fixture in load_fixtures() {
        let Fixture::Secretstream {
            label,
            key,
            header,
            messages,
            ..
        } = fixture
        else {
            continue;
        };

        let key_bytes = h32(&key);
        let header_bytes: [u8; HEADERBYTES] = h(&header)
            .try_into()
            .unwrap_or_else(|v: Vec<u8>| panic!("{label}: header len {}", v.len()));

        let mut push = Push::with_header(&key_bytes, &header_bytes);

        for (i, msg) in messages.iter().enumerate() {
            let pt = h(&msg.plaintext);
            let tag = match msg.tag.as_str() {
                "message" => TAG_MESSAGE,
                "final" => TAG_FINAL,
                other => panic!("{label} msg[{i}]: unknown tag '{other}'"),
            };

            let ct = push.push(&pt, None, tag);
            let expected_ct = h(&msg.ciphertext);
            assert_eq!(
                hex::encode(&ct),
                hex::encode(&expected_ct),
                "{label} msg[{i}]: ciphertext mismatch"
            );
        }
    }
}
