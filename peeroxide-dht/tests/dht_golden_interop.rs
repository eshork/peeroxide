use peeroxide_dht::messages::{
    decode_message, encode_request_to_bytes, encode_response_to_bytes, Ipv4Peer, Message, Request,
    Response,
};
use peeroxide_dht::peer::peer_id;
use serde::Deserialize;

#[derive(Deserialize)]
struct FixtureFile {
    #[allow(dead_code)]
    generator: String,
    #[allow(dead_code)]
    dht_rpc_version: String,
    fixtures: Vec<Fixture>,
}

#[derive(Deserialize)]
struct Fixture {
    #[serde(rename = "type")]
    typ: String,
    label: String,
    fields: serde_json::Value,
    hex: String,
}

fn load_fixtures() -> Vec<Fixture> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tests/interop/dht-rpc-fixtures.json"
    );
    let data = std::fs::read_to_string(path).unwrap_or_else(|e| {
        panic!("Failed to read dht-rpc fixtures at {path}: {e}. Run `node generate-dht-golden.js` in tests/node/ first.")
    });
    let file: FixtureFile = serde_json::from_str(&data)
        .unwrap_or_else(|e| panic!("Failed to parse dht-rpc fixtures: {e}"));
    file.fixtures
}

fn hex_bytes(hex_str: &str) -> Vec<u8> {
    hex::decode(hex_str).unwrap_or_else(|e| panic!("Invalid hex '{hex_str}': {e}"))
}

fn opt_fixed32(v: &serde_json::Value) -> Option<[u8; 32]> {
    if v.is_null() {
        return None;
    }
    let s = v.as_str().expect("expected hex string for fixed32 field");
    let bytes = hex::decode(s).expect("valid hex for fixed32");
    let arr: [u8; 32] = bytes.try_into().expect("expected exactly 32 bytes");
    Some(arr)
}

fn opt_bytes(v: &serde_json::Value) -> Option<Vec<u8>> {
    if v.is_null() {
        return None;
    }
    let s = v.as_str().expect("expected hex string for bytes field");
    if s.is_empty() {
        return None;
    }
    Some(hex::decode(s).expect("valid hex for bytes field"))
}

#[test]
fn golden_requests() {
    for f in load_fixtures().iter().filter(|f| f.typ == "request") {
        let expected = hex_bytes(&f.hex);
        let flds = &f.fields;

        let tid = flds["tid"].as_u64().expect("tid") as u16;
        let to_host = flds["to_host"].as_str().expect("to_host").to_string();
        let to_port = flds["to_port"].as_u64().expect("to_port") as u16;
        let id = opt_fixed32(&flds["id"]);
        let token = opt_fixed32(&flds["token"]);
        let internal = flds["internal"].as_bool().expect("internal");
        let command = flds["command"].as_u64().expect("command");
        let target = opt_fixed32(&flds["target"]);
        let value = opt_bytes(&flds["value"]);

        let msg = decode_message(&expected)
            .unwrap_or_else(|e| panic!("DECODE {}: {e}", f.label));
        let Message::Request(decoded) = msg else {
            panic!("Expected Request for fixture {}", f.label);
        };

        assert_eq!(decoded.tid, tid, "tid mismatch: {}", f.label);
        assert_eq!(decoded.to.host, to_host, "to.host mismatch: {}", f.label);
        assert_eq!(decoded.to.port, to_port, "to.port mismatch: {}", f.label);
        assert_eq!(decoded.id, id, "id mismatch: {}", f.label);
        assert_eq!(decoded.token, token, "token mismatch: {}", f.label);
        assert_eq!(decoded.internal, internal, "internal mismatch: {}", f.label);
        assert_eq!(decoded.command, command, "command mismatch: {}", f.label);
        assert_eq!(decoded.target, target, "target mismatch: {}", f.label);
        assert_eq!(decoded.value, value, "value mismatch: {}", f.label);

        let req = Request {
            tid,
            to: Ipv4Peer {
                host: to_host,
                port: to_port,
            },
            id,
            token,
            internal,
            command,
            target,
            value,
        };
        let encoded = encode_request_to_bytes(&req)
            .unwrap_or_else(|e| panic!("ENCODE {}: {e}", f.label));
        assert_eq!(
            encoded,
            expected,
            "ENCODE byte mismatch {}: expected {}, got {}",
            f.label,
            f.hex,
            hex::encode(&encoded)
        );
    }
}

#[test]
fn golden_responses() {
    for f in load_fixtures().iter().filter(|f| f.typ == "response") {
        let expected = hex_bytes(&f.hex);
        let flds = &f.fields;

        let tid = flds["tid"].as_u64().expect("tid") as u16;
        let to_host = flds["to_host"].as_str().expect("to_host").to_string();
        let to_port = flds["to_port"].as_u64().expect("to_port") as u16;
        let id = opt_fixed32(&flds["id"]);
        let token = opt_fixed32(&flds["token"]);
        let closer_nodes: Vec<Ipv4Peer> = flds["closer_nodes"]
            .as_array()
            .expect("closer_nodes array")
            .iter()
            .map(|n| Ipv4Peer {
                host: n["host"].as_str().expect("closer node host").to_string(),
                port: n["port"].as_u64().expect("closer node port") as u16,
            })
            .collect();
        let error = flds["error"].as_u64().expect("error");
        let value = opt_bytes(&flds["value"]);

        let msg = decode_message(&expected)
            .unwrap_or_else(|e| panic!("DECODE {}: {e}", f.label));
        let Message::Response(decoded) = msg else {
            panic!("Expected Response for fixture {}", f.label);
        };

        assert_eq!(decoded.tid, tid, "tid mismatch: {}", f.label);
        assert_eq!(decoded.to.host, to_host, "to.host mismatch: {}", f.label);
        assert_eq!(decoded.to.port, to_port, "to.port mismatch: {}", f.label);
        assert_eq!(decoded.id, id, "id mismatch: {}", f.label);
        assert_eq!(decoded.token, token, "token mismatch: {}", f.label);
        assert_eq!(
            decoded.closer_nodes.len(),
            closer_nodes.len(),
            "closer_nodes length mismatch: {}",
            f.label
        );
        for (i, (got, want)) in decoded
            .closer_nodes
            .iter()
            .zip(closer_nodes.iter())
            .enumerate()
        {
            assert_eq!(
                got.host, want.host,
                "closer_nodes[{i}].host mismatch: {}",
                f.label
            );
            assert_eq!(
                got.port, want.port,
                "closer_nodes[{i}].port mismatch: {}",
                f.label
            );
        }
        assert_eq!(decoded.error, error, "error mismatch: {}", f.label);
        assert_eq!(decoded.value, value, "value mismatch: {}", f.label);

        let res = Response {
            tid,
            to: Ipv4Peer {
                host: to_host,
                port: to_port,
            },
            id,
            token,
            closer_nodes,
            error,
            value,
        };
        let encoded = encode_response_to_bytes(&res)
            .unwrap_or_else(|e| panic!("ENCODE {}: {e}", f.label));
        assert_eq!(
            encoded,
            expected,
            "ENCODE byte mismatch {}: expected {}, got {}",
            f.label,
            f.hex,
            hex::encode(&encoded)
        );
    }
}

#[test]
fn golden_peer_id() {
    for f in load_fixtures().iter().filter(|f| f.typ == "peer_id") {
        let expected = hex_bytes(&f.hex);
        let host = f.fields["host"].as_str().expect("host");
        let port = f.fields["port"].as_u64().expect("port") as u16;
        let id = peer_id(host, port);
        assert_eq!(
            id.as_ref(),
            expected.as_slice(),
            "peer_id mismatch {}: expected {}, got {}",
            f.label,
            f.hex,
            hex::encode(id)
        );
    }
}
