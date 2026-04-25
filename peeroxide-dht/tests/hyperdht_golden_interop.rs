use peeroxide_dht::crypto::{
    NS_ANNOUNCE, NS_MUTABLE_PUT, NS_PEER_HANDSHAKE, NS_PEER_HOLEPUNCH, NS_UNANNOUNCE,
};
use peeroxide_dht::hyperdht_messages::{
    decode_announce_from_bytes, decode_handshake_from_bytes, decode_holepunch_msg_from_bytes,
    decode_holepunch_payload_from_bytes, decode_hyper_peer_from_bytes,
    decode_lookup_raw_reply_from_bytes, decode_mutable_get_response_from_bytes,
    decode_mutable_put_request_from_bytes, decode_mutable_signable_from_bytes,
    decode_noise_payload_from_bytes, encode_announce_to_bytes, encode_handshake_to_bytes,
    encode_holepunch_msg_to_bytes, encode_holepunch_payload_to_bytes,
    encode_hyper_peer_to_bytes, encode_lookup_raw_reply_to_bytes,
    encode_mutable_get_response_to_bytes, encode_mutable_put_request_to_bytes,
    encode_mutable_signable_to_bytes, encode_noise_payload_to_bytes, AnnounceMessage,
    HandshakeMessage, HolepunchInfo, HolepunchMessage, HolepunchPayload, HyperPeer,
    LookupRawReply, MutableGetResponse, MutablePutRequest, MutableSignable, NoisePayload,
    RelayInfo, RelayThroughInfo, SecretStreamInfo, UdxInfo,
};
use peeroxide_dht::messages::Ipv4Peer;
use serde::Deserialize;

#[derive(Deserialize)]
struct FixtureFile {
    #[allow(dead_code)]
    generator: String,
    #[allow(dead_code)]
    hyperdht_version: String,
    fixtures: Vec<Fixture>,
}

#[derive(Deserialize)]
struct Fixture {
    #[serde(rename = "type")]
    typ: String,
    label: String,
    #[serde(default)]
    fields: serde_json::Value,
    hex: String,
}

fn load_fixtures() -> Vec<Fixture> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tests/interop/hyperdht-fixtures.json"
    );
    let data = std::fs::read_to_string(path).unwrap_or_else(|e| {
        panic!("Failed to read hyperdht fixtures at {path}: {e}. Run `node generate-hyperdht-golden.js` in tests/node/ first.")
    });
    let file: FixtureFile = serde_json::from_str(&data)
        .unwrap_or_else(|e| panic!("Failed to parse hyperdht fixtures: {e}"));
    file.fixtures
}

fn hex_bytes(hex_str: &str) -> Vec<u8> {
    hex::decode(hex_str).unwrap_or_else(|e| panic!("Invalid hex '{hex_str}': {e}"))
}

fn opt_hex32(v: &serde_json::Value) -> Option<[u8; 32]> {
    if v.is_null() {
        return None;
    }
    let s = v.as_str().expect("hex string");
    let b = hex::decode(s).expect("valid hex");
    Some(b.try_into().expect("32 bytes"))
}

fn opt_hex64(v: &serde_json::Value) -> Option<[u8; 64]> {
    if v.is_null() {
        return None;
    }
    let s = v.as_str().expect("hex string");
    let b = hex::decode(s).expect("valid hex");
    Some(b.try_into().expect("64 bytes"))
}

fn peer_from_fields(f: &serde_json::Value) -> HyperPeer {
    let pk = hex::decode(f["publicKey"].as_str().expect("publicKey hex"))
        .expect("valid hex");
    let public_key: [u8; 32] = pk.try_into().expect("32 bytes");
    let relay_addresses = f["relayAddresses"]
        .as_array()
        .expect("relayAddresses array")
        .iter()
        .map(|r| Ipv4Peer {
            host: r["host"].as_str().expect("relay host").to_string(),
            port: r["port"].as_u64().expect("relay port") as u16,
        })
        .collect();
    HyperPeer {
        public_key,
        relay_addresses,
    }
}

#[test]
fn golden_hyper_peer() {
    for f in load_fixtures().iter().filter(|f| f.typ == "hyper_peer") {
        let expected = hex_bytes(&f.hex);
        let peer = peer_from_fields(&f.fields);

        let encoded = encode_hyper_peer_to_bytes(&peer)
            .unwrap_or_else(|e| panic!("ENCODE {}: {e}", f.label));
        assert_eq!(
            encoded,
            expected,
            "ENCODE mismatch {}: expected {}, got {}",
            f.label,
            f.hex,
            hex::encode(&encoded)
        );

        let decoded = decode_hyper_peer_from_bytes(&expected)
            .unwrap_or_else(|e| panic!("DECODE {}: {e}", f.label));
        assert_eq!(
            decoded.public_key,
            peer.public_key,
            "publicKey mismatch: {}",
            f.label
        );
        assert_eq!(
            decoded.relay_addresses.len(),
            peer.relay_addresses.len(),
            "relay count mismatch: {}",
            f.label
        );
        for (i, (got, want)) in decoded
            .relay_addresses
            .iter()
            .zip(peer.relay_addresses.iter())
            .enumerate()
        {
            assert_eq!(got.host, want.host, "relay[{i}].host: {}", f.label);
            assert_eq!(got.port, want.port, "relay[{i}].port: {}", f.label);
        }
    }
}

#[test]
fn golden_announce() {
    for f in load_fixtures().iter().filter(|f| f.typ == "announce") {
        let expected = hex_bytes(&f.hex);
        let flds = &f.fields;

        let peer = if flds["peer"].is_null() {
            None
        } else {
            Some(peer_from_fields(&flds["peer"]))
        };
        let refresh = opt_hex32(&flds["refresh"]);
        let signature = opt_hex64(&flds["signature"]);
        let bump = flds["bump"].as_u64().expect("bump");

        let m = AnnounceMessage {
            peer,
            refresh,
            signature,
            bump,
        };

        let encoded = encode_announce_to_bytes(&m)
            .unwrap_or_else(|e| panic!("ENCODE {}: {e}", f.label));
        assert_eq!(
            encoded,
            expected,
            "ENCODE mismatch {}: expected {}, got {}",
            f.label,
            f.hex,
            hex::encode(&encoded)
        );

        let decoded = decode_announce_from_bytes(&expected)
            .unwrap_or_else(|e| panic!("DECODE {}: {e}", f.label));
        assert_eq!(
            decoded.refresh,
            m.refresh,
            "refresh mismatch: {}",
            f.label
        );
        assert_eq!(
            decoded.signature,
            m.signature,
            "signature mismatch: {}",
            f.label
        );
        assert_eq!(decoded.bump, m.bump, "bump mismatch: {}", f.label);
        assert_eq!(
            decoded.peer.is_some(),
            m.peer.is_some(),
            "peer presence mismatch: {}",
            f.label
        );
        if let (Some(dp), Some(mp)) = (decoded.peer, m.peer) {
            assert_eq!(dp.public_key, mp.public_key, "peer.pk mismatch: {}", f.label);
        }
    }
}

#[test]
fn golden_lookup_raw_reply() {
    for f in load_fixtures()
        .iter()
        .filter(|f| f.typ == "lookup_raw_reply")
    {
        let expected = hex_bytes(&f.hex);
        let flds = &f.fields;

        let peers: Vec<HyperPeer> = flds["peers"]
            .as_array()
            .expect("peers array")
            .iter()
            .map(peer_from_fields)
            .collect();
        let bump = flds["bump"].as_u64().expect("bump");

        let m = LookupRawReply {
            peers,
            bump,
        };

        let encoded = encode_lookup_raw_reply_to_bytes(&m)
            .unwrap_or_else(|e| panic!("ENCODE {}: {e}", f.label));
        assert_eq!(
            encoded,
            expected,
            "ENCODE mismatch {}: expected {}, got {}",
            f.label,
            f.hex,
            hex::encode(&encoded)
        );

        let decoded = decode_lookup_raw_reply_from_bytes(&expected)
            .unwrap_or_else(|e| panic!("DECODE {}: {e}", f.label));
        assert_eq!(
            decoded.peers.len(),
            m.peers.len(),
            "peer count mismatch: {}",
            f.label
        );
        assert_eq!(decoded.bump, m.bump, "bump mismatch: {}", f.label);
        for (i, (got, want)) in decoded.peers.iter().zip(m.peers.iter()).enumerate() {
            assert_eq!(
                got.public_key,
                want.public_key,
                "peers[{i}].pk mismatch: {}",
                f.label
            );
        }
    }
}

#[test]
fn golden_mutable_put_request() {
    for f in load_fixtures()
        .iter()
        .filter(|f| f.typ == "mutable_put_request")
    {
        let expected = hex_bytes(&f.hex);
        let flds = &f.fields;

        let pk_bytes = hex::decode(flds["publicKey"].as_str().expect("publicKey")).unwrap();
        let public_key: [u8; 32] = pk_bytes.try_into().expect("32 bytes");
        let seq = flds["seq"].as_u64().expect("seq");
        let value = hex::decode(flds["value"].as_str().expect("value")).unwrap();
        let sig_bytes = hex::decode(flds["signature"].as_str().expect("signature")).unwrap();
        let signature: [u8; 64] = sig_bytes.try_into().expect("64 bytes");

        let m = MutablePutRequest {
            public_key,
            seq,
            value,
            signature,
        };

        let encoded = encode_mutable_put_request_to_bytes(&m)
            .unwrap_or_else(|e| panic!("ENCODE {}: {e}", f.label));
        assert_eq!(
            encoded,
            expected,
            "ENCODE mismatch {}: expected {}, got {}",
            f.label,
            f.hex,
            hex::encode(&encoded)
        );

        let decoded = decode_mutable_put_request_from_bytes(&expected)
            .unwrap_or_else(|e| panic!("DECODE {}: {e}", f.label));
        assert_eq!(decoded, m, "decode mismatch: {}", f.label);
    }
}

#[test]
fn golden_mutable_get_response() {
    for f in load_fixtures()
        .iter()
        .filter(|f| f.typ == "mutable_get_response")
    {
        let expected = hex_bytes(&f.hex);
        let flds = &f.fields;

        let seq = flds["seq"].as_u64().expect("seq");
        let value = hex::decode(flds["value"].as_str().expect("value")).unwrap();
        let sig_bytes = hex::decode(flds["signature"].as_str().expect("signature")).unwrap();
        let signature: [u8; 64] = sig_bytes.try_into().expect("64 bytes");

        let m = MutableGetResponse {
            seq,
            value,
            signature,
        };

        let encoded = encode_mutable_get_response_to_bytes(&m)
            .unwrap_or_else(|e| panic!("ENCODE {}: {e}", f.label));
        assert_eq!(
            encoded,
            expected,
            "ENCODE mismatch {}: expected {}, got {}",
            f.label,
            f.hex,
            hex::encode(&encoded)
        );

        let decoded = decode_mutable_get_response_from_bytes(&expected)
            .unwrap_or_else(|e| panic!("DECODE {}: {e}", f.label));
        assert_eq!(decoded, m, "decode mismatch: {}", f.label);
    }
}

#[test]
fn golden_mutable_signable() {
    for f in load_fixtures()
        .iter()
        .filter(|f| f.typ == "mutable_signable")
    {
        let expected = hex_bytes(&f.hex);
        let flds = &f.fields;

        let seq = flds["seq"].as_u64().expect("seq");
        let value = hex::decode(flds["value"].as_str().expect("value")).unwrap();

        let m = MutableSignable { seq, value };

        let encoded = encode_mutable_signable_to_bytes(&m)
            .unwrap_or_else(|e| panic!("ENCODE {}: {e}", f.label));
        assert_eq!(
            encoded,
            expected,
            "ENCODE mismatch {}: expected {}, got {}",
            f.label,
            f.hex,
            hex::encode(&encoded)
        );

        let decoded = decode_mutable_signable_from_bytes(&expected)
            .unwrap_or_else(|e| panic!("DECODE {}: {e}", f.label));
        assert_eq!(decoded, m, "decode mismatch: {}", f.label);
    }
}

#[test]
fn golden_namespaces() {
    let ns_map = [
        ("NS_ANNOUNCE", &*NS_ANNOUNCE),
        ("NS_UNANNOUNCE", &*NS_UNANNOUNCE),
        ("NS_MUTABLE_PUT", &*NS_MUTABLE_PUT),
        ("NS_PEER_HANDSHAKE", &*NS_PEER_HANDSHAKE),
        ("NS_PEER_HOLEPUNCH", &*NS_PEER_HOLEPUNCH),
    ];

    for f in load_fixtures().iter().filter(|f| f.typ == "namespace") {
        let expected = hex_bytes(&f.hex);
        let label = f.label.as_str();

        let rust_ns = ns_map
            .iter()
            .find(|(k, _)| *k == label)
            .unwrap_or_else(|| panic!("Unknown namespace fixture: {label}"));

        assert_eq!(
            rust_ns.1.as_ref(),
            expected.as_slice(),
            "namespace mismatch {}: expected {}, got {}",
            label,
            f.hex,
            hex::encode(rust_ns.1)
        );
    }
}

fn ipv4_peer_from_json(v: &serde_json::Value) -> Ipv4Peer {
    Ipv4Peer {
        host: v["host"].as_str().expect("host").to_string(),
        port: v["port"].as_u64().expect("port") as u16,
    }
}

fn opt_ipv4_peer_from_json(v: &serde_json::Value) -> Option<Ipv4Peer> {
    if v.is_null() {
        None
    } else {
        Some(ipv4_peer_from_json(v))
    }
}

#[test]
fn golden_handshake() {
    for f in load_fixtures().iter().filter(|f| f.typ == "handshake") {
        let expected = hex_bytes(&f.hex);
        let flds = &f.fields;

        let m = HandshakeMessage {
            mode: flds["mode"].as_u64().expect("mode"),
            noise: hex::decode(flds["noise"].as_str().expect("noise")).unwrap(),
            peer_address: opt_ipv4_peer_from_json(&flds["peerAddress"]),
            relay_address: opt_ipv4_peer_from_json(&flds["relayAddress"]),
        };

        let encoded = encode_handshake_to_bytes(&m)
            .unwrap_or_else(|e| panic!("ENCODE {}: {e}", f.label));
        assert_eq!(
            encoded, expected,
            "ENCODE mismatch {}: expected {}, got {}",
            f.label, f.hex, hex::encode(&encoded)
        );

        let decoded = decode_handshake_from_bytes(&expected)
            .unwrap_or_else(|e| panic!("DECODE {}: {e}", f.label));
        assert_eq!(decoded, m, "DECODE mismatch: {}", f.label);
    }
}

#[test]
fn golden_holepunch_msg() {
    for f in load_fixtures().iter().filter(|f| f.typ == "holepunch") {
        let expected = hex_bytes(&f.hex);
        let flds = &f.fields;

        let m = HolepunchMessage {
            mode: flds["mode"].as_u64().expect("mode"),
            id: flds["id"].as_u64().expect("id"),
            payload: hex::decode(flds["payload"].as_str().expect("payload")).unwrap(),
            peer_address: opt_ipv4_peer_from_json(&flds["peerAddress"]),
        };

        let encoded = encode_holepunch_msg_to_bytes(&m)
            .unwrap_or_else(|e| panic!("ENCODE {}: {e}", f.label));
        assert_eq!(
            encoded, expected,
            "ENCODE mismatch {}: expected {}, got {}",
            f.label, f.hex, hex::encode(&encoded)
        );

        let decoded = decode_holepunch_msg_from_bytes(&expected)
            .unwrap_or_else(|e| panic!("DECODE {}: {e}", f.label));
        assert_eq!(decoded, m, "DECODE mismatch: {}", f.label);
    }
}

fn ipv4_peers_from_json(v: &serde_json::Value) -> Vec<Ipv4Peer> {
    v.as_array()
        .map(|arr| arr.iter().map(ipv4_peer_from_json).collect())
        .unwrap_or_default()
}

fn relay_info_from_json(v: &serde_json::Value) -> RelayInfo {
    RelayInfo {
        relay_address: ipv4_peer_from_json(&v["relayAddress"]),
        peer_address: ipv4_peer_from_json(&v["peerAddress"]),
    }
}

#[test]
fn golden_noise_payload() {
    for f in load_fixtures()
        .iter()
        .filter(|f| f.typ == "noise_payload")
    {
        let expected = hex_bytes(&f.hex);
        let flds = &f.fields;

        let holepunch = if flds["holepunch"].is_null() {
            None
        } else {
            let hp = &flds["holepunch"];
            Some(HolepunchInfo {
                id: hp["id"].as_u64().expect("hp.id"),
                relays: hp["relays"]
                    .as_array()
                    .expect("relays array")
                    .iter()
                    .map(relay_info_from_json)
                    .collect(),
            })
        };

        let udx = if flds["udx"].is_null() {
            None
        } else {
            let u = &flds["udx"];
            Some(UdxInfo {
                version: u["version"].as_u64().expect("udx.version"),
                reusable_socket: u["reusableSocket"].as_bool().expect("udx.reusableSocket"),
                id: u["id"].as_u64().expect("udx.id"),
                seq: u["seq"].as_u64().expect("udx.seq"),
            })
        };

        let secret_stream = if flds["secretStream"].is_null() {
            None
        } else {
            Some(SecretStreamInfo {
                version: flds["secretStream"]["version"]
                    .as_u64()
                    .expect("ss.version"),
            })
        };

        let relay_through = if flds["relayThrough"].is_null() {
            None
        } else {
            let rt = &flds["relayThrough"];
            let pk = hex::decode(rt["publicKey"].as_str().expect("rt.pk")).unwrap();
            let tok = hex::decode(rt["token"].as_str().expect("rt.token")).unwrap();
            Some(RelayThroughInfo {
                version: rt["version"].as_u64().expect("rt.version"),
                public_key: pk.try_into().expect("32 bytes"),
                token: tok.try_into().expect("32 bytes"),
            })
        };

        let relay_addresses = if flds["relayAddresses"].is_null() {
            None
        } else {
            Some(ipv4_peers_from_json(&flds["relayAddresses"]))
        };

        let m = NoisePayload {
            version: flds["version"].as_u64().expect("version"),
            error: flds["error"].as_u64().expect("error"),
            firewall: flds["firewall"].as_u64().expect("firewall"),
            holepunch,
            addresses4: ipv4_peers_from_json(&flds["addresses4"]),
            addresses6: ipv4_peers_from_json(&flds["addresses6"]),
            udx,
            secret_stream,
            relay_through,
            relay_addresses,
        };

        let encoded = encode_noise_payload_to_bytes(&m)
            .unwrap_or_else(|e| panic!("ENCODE {}: {e}", f.label));
        assert_eq!(
            encoded, expected,
            "ENCODE mismatch {}: expected {}, got {}",
            f.label, f.hex, hex::encode(&encoded)
        );

        let decoded = decode_noise_payload_from_bytes(&expected)
            .unwrap_or_else(|e| panic!("DECODE {}: {e}", f.label));
        assert_eq!(decoded, m, "DECODE mismatch: {}", f.label);
    }
}

#[test]
fn golden_holepunch_payload() {
    for f in load_fixtures()
        .iter()
        .filter(|f| f.typ == "holepunch_payload")
    {
        let expected = hex_bytes(&f.hex);
        let flds = &f.fields;

        let addresses = if flds["addresses"].is_null() {
            None
        } else {
            Some(ipv4_peers_from_json(&flds["addresses"]))
        };

        let token = if flds["token"].is_null() {
            None
        } else {
            let b = hex::decode(flds["token"].as_str().expect("token hex")).unwrap();
            Some(<[u8; 32]>::try_from(b.as_slice()).expect("32 bytes"))
        };

        let remote_token = if flds["remoteToken"].is_null() {
            None
        } else {
            let b = hex::decode(flds["remoteToken"].as_str().expect("remoteToken hex")).unwrap();
            Some(<[u8; 32]>::try_from(b.as_slice()).expect("32 bytes"))
        };

        let m = HolepunchPayload {
            error: flds["error"].as_u64().expect("error"),
            firewall: flds["firewall"].as_u64().expect("firewall"),
            round: flds["round"].as_u64().expect("round"),
            connected: flds["connected"].as_bool().expect("connected"),
            punching: flds["punching"].as_bool().expect("punching"),
            addresses,
            remote_address: opt_ipv4_peer_from_json(&flds["remoteAddress"]),
            token,
            remote_token,
        };

        let encoded = encode_holepunch_payload_to_bytes(&m)
            .unwrap_or_else(|e| panic!("ENCODE {}: {e}", f.label));
        assert_eq!(
            encoded, expected,
            "ENCODE mismatch {}: expected {}, got {}",
            f.label, f.hex, hex::encode(&encoded)
        );

        let decoded = decode_holepunch_payload_from_bytes(&expected)
            .unwrap_or_else(|e| panic!("DECODE {}: {e}", f.label));
        assert_eq!(decoded, m, "DECODE mismatch: {}", f.label);
    }
}
