use peeroxide_dht::blind_relay::{
    decode_pair_from_slice, decode_unpair_from_slice, encode_pair_to_vec, encode_unpair_to_vec,
    PairMessage, UnpairMessage,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct GoldenFile {
    #[allow(dead_code)]
    generated_by: String,
    #[allow(dead_code)]
    blind_relay_version: String,
    fixtures: Vec<Fixture>,
}

#[derive(Deserialize)]
struct Fixture {
    label: String,
    #[serde(rename = "type")]
    typ: String,
    hex: String,
    decoded: serde_json::Value,
}

fn load_fixtures() -> Vec<Fixture> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tests/interop/blind-relay-fixtures.json"
    );
    let data = std::fs::read_to_string(path).unwrap_or_else(|e| {
        panic!("Failed to read blind-relay fixtures at {path}: {e}. Run `node generate-blind-relay-golden.js` in tests/node/ first.")
    });
    let file: GoldenFile = serde_json::from_str(&data)
        .unwrap_or_else(|e| panic!("Failed to parse blind-relay fixtures: {e}"));
    file.fixtures
}

fn from_hex(s: &str) -> Vec<u8> {
    hex::decode(s).unwrap_or_else(|e| panic!("Invalid hex '{s}': {e}"))
}

fn token_from_hex(s: &str) -> [u8; 32] {
    let v = from_hex(s);
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&v);
    arr
}

#[test]
fn golden_blind_relay_decode_pair() {
    let fixtures = load_fixtures();
    let pairs: Vec<_> = fixtures.iter().filter(|f| f.typ == "pair").collect();
    assert!(!pairs.is_empty(), "no pair fixtures found");

    for fix in pairs {
        let raw = from_hex(&fix.hex);
        let decoded = decode_pair_from_slice(&raw)
            .unwrap_or_else(|e| panic!("[{}] decode failed: {e}", fix.label));

        let d = &fix.decoded;
        let expected = PairMessage {
            is_initiator: d["is_initiator"].as_bool().unwrap(),
            token: token_from_hex(d["token"].as_str().unwrap()),
            id: d["id"].as_u64().unwrap(),
            seq: d["seq"].as_u64().unwrap(),
        };

        assert_eq!(decoded, expected, "[{}] decode mismatch", fix.label);
    }
}

#[test]
fn golden_blind_relay_decode_unpair() {
    let fixtures = load_fixtures();
    let unpairs: Vec<_> = fixtures.iter().filter(|f| f.typ == "unpair").collect();
    assert!(!unpairs.is_empty(), "no unpair fixtures found");

    for fix in unpairs {
        let raw = from_hex(&fix.hex);
        let decoded = decode_unpair_from_slice(&raw)
            .unwrap_or_else(|e| panic!("[{}] decode failed: {e}", fix.label));

        let expected = UnpairMessage {
            token: token_from_hex(fix.decoded["token"].as_str().unwrap()),
        };

        assert_eq!(decoded, expected, "[{}] decode mismatch", fix.label);
    }
}

#[test]
fn golden_blind_relay_encode_roundtrip() {
    let fixtures = load_fixtures();

    for fix in &fixtures {
        let expected_bytes = from_hex(&fix.hex);
        let d = &fix.decoded;

        let encoded = match fix.typ.as_str() {
            "pair" => {
                let msg = PairMessage {
                    is_initiator: d["is_initiator"].as_bool().unwrap(),
                    token: token_from_hex(d["token"].as_str().unwrap()),
                    id: d["id"].as_u64().unwrap(),
                    seq: d["seq"].as_u64().unwrap(),
                };
                encode_pair_to_vec(&msg)
            }
            "unpair" => {
                let msg = UnpairMessage {
                    token: token_from_hex(d["token"].as_str().unwrap()),
                };
                encode_unpair_to_vec(&msg)
            }
            other => panic!("[{}] unknown fixture type: {other}", fix.label),
        };

        assert_eq!(
            encoded, expected_bytes,
            "[{}] encode roundtrip mismatch\n  encoded: {}\n  expected: {}",
            fix.label,
            hex::encode(&encoded),
            fix.hex,
        );
    }
}
