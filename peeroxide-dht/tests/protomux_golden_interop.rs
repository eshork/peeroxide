use peeroxide_dht::protomux::{
    decode_frame, encode_batch, encode_close, encode_message, encode_open, encode_reject,
    ControlFrame, DecodedFrame,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct GoldenFile {
    #[allow(dead_code)]
    generated_by: String,
    #[allow(dead_code)]
    protomux_version: String,
    #[allow(dead_code)]
    compact_encoding_version: String,
    fixtures: Fixtures,
}

#[derive(Deserialize)]
struct Fixtures {
    frames: Vec<FrameFixture>,
    conversations: Vec<ConversationFixture>,
}

#[derive(Deserialize)]
struct FrameFixture {
    label: String,
    #[serde(rename = "type")]
    typ: String,
    hex: String,
    decoded: serde_json::Value,
}

#[derive(Deserialize)]
struct ConversationFixture {
    label: String,
    #[allow(dead_code)]
    protocol: String,
    a_to_b: Vec<String>,
    b_to_a: Vec<String>,
}

fn load_fixtures() -> GoldenFile {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tests/interop/protomux-fixtures.json"
    );
    let data = std::fs::read_to_string(path).unwrap_or_else(|e| {
        panic!("Failed to read protomux fixtures at {path}: {e}. Run `node generate-protomux-golden.js` in tests/node/ first.")
    });
    serde_json::from_str(&data)
        .unwrap_or_else(|e| panic!("Failed to parse protomux fixtures: {e}"))
}

fn from_hex(s: &str) -> Vec<u8> {
    hex::decode(s).unwrap_or_else(|e| panic!("Invalid hex '{s}': {e}"))
}

fn opt_hex(v: &serde_json::Value) -> Option<Vec<u8>> {
    match v {
        serde_json::Value::Null => None,
        serde_json::Value::String(s) => Some(from_hex(s)),
        _ => panic!("Expected null or hex string, got {v:?}"),
    }
}

#[test]
fn golden_protomux_decode_open_frames() {
    let file = load_fixtures();
    let opens: Vec<_> = file
        .fixtures
        .frames
        .iter()
        .filter(|f| f.typ == "open")
        .collect();
    assert!(!opens.is_empty(), "no open fixtures found");

    for fix in opens {
        let raw = from_hex(&fix.hex);
        let decoded = decode_frame(&raw).unwrap_or_else(|e| {
            panic!("[{}] decode failed: {e}", fix.label);
        });

        let d = &fix.decoded;
        let expected_local_id = d["local_id"].as_u64().unwrap();
        let expected_protocol = d["protocol"].as_str().unwrap();
        let expected_id = opt_hex(&d["id"]);
        let expected_handshake = opt_hex(&d["handshake"]);

        match decoded {
            DecodedFrame::Control(ControlFrame::Open {
                local_id,
                protocol,
                id,
                handshake_state,
            }) => {
                assert_eq!(
                    local_id, expected_local_id,
                    "[{}] local_id mismatch",
                    fix.label
                );
                assert_eq!(
                    protocol, expected_protocol,
                    "[{}] protocol mismatch",
                    fix.label
                );
                assert_eq!(id, expected_id, "[{}] id mismatch", fix.label);
                assert_eq!(
                    handshake_state, expected_handshake,
                    "[{}] handshake mismatch",
                    fix.label
                );
            }
            other => panic!("[{}] expected Open, got {other:?}", fix.label),
        }
    }
}

#[test]
fn golden_protomux_decode_close_frames() {
    let file = load_fixtures();
    let closes: Vec<_> = file
        .fixtures
        .frames
        .iter()
        .filter(|f| f.typ == "close")
        .collect();
    assert!(!closes.is_empty(), "no close fixtures found");

    for fix in closes {
        let raw = from_hex(&fix.hex);
        let decoded = decode_frame(&raw).unwrap_or_else(|e| {
            panic!("[{}] decode failed: {e}", fix.label);
        });

        let expected_local_id = fix.decoded["local_id"].as_u64().unwrap();

        match decoded {
            DecodedFrame::Control(ControlFrame::Close { local_id }) => {
                assert_eq!(
                    local_id, expected_local_id,
                    "[{}] local_id mismatch",
                    fix.label
                );
            }
            other => panic!("[{}] expected Close, got {other:?}", fix.label),
        }
    }
}

#[test]
fn golden_protomux_decode_reject_frames() {
    let file = load_fixtures();
    let rejects: Vec<_> = file
        .fixtures
        .frames
        .iter()
        .filter(|f| f.typ == "reject")
        .collect();
    assert!(!rejects.is_empty(), "no reject fixtures found");

    for fix in rejects {
        let raw = from_hex(&fix.hex);
        let decoded = decode_frame(&raw).unwrap_or_else(|e| {
            panic!("[{}] decode failed: {e}", fix.label);
        });

        let expected_remote_id = fix.decoded["remote_id"].as_u64().unwrap();

        match decoded {
            DecodedFrame::Control(ControlFrame::Reject { remote_id }) => {
                assert_eq!(
                    remote_id, expected_remote_id,
                    "[{}] remote_id mismatch",
                    fix.label
                );
            }
            other => panic!("[{}] expected Reject, got {other:?}", fix.label),
        }
    }
}

#[test]
fn golden_protomux_decode_message_frames() {
    let file = load_fixtures();
    let msgs: Vec<_> = file
        .fixtures
        .frames
        .iter()
        .filter(|f| f.typ == "message")
        .collect();
    assert!(!msgs.is_empty(), "no message fixtures found");

    for fix in msgs {
        let raw = from_hex(&fix.hex);
        let decoded = decode_frame(&raw).unwrap_or_else(|e| {
            panic!("[{}] decode failed: {e}", fix.label);
        });

        let d = &fix.decoded;
        let expected_channel_id = d["channel_id"].as_u64().unwrap();
        let expected_message_type = d["message_type"].as_u64().unwrap();
        let expected_payload = from_hex(d["payload"].as_str().unwrap());

        match decoded {
            DecodedFrame::Message {
                channel_id,
                message_type,
                payload,
            } => {
                assert_eq!(
                    channel_id, expected_channel_id,
                    "[{}] channel_id mismatch",
                    fix.label
                );
                assert_eq!(
                    message_type, expected_message_type,
                    "[{}] message_type mismatch",
                    fix.label
                );
                assert_eq!(
                    payload, expected_payload,
                    "[{}] payload mismatch",
                    fix.label
                );
            }
            other => panic!("[{}] expected Message, got {other:?}", fix.label),
        }
    }
}

#[test]
fn golden_protomux_decode_batch_frames() {
    let file = load_fixtures();
    let batches: Vec<_> = file
        .fixtures
        .frames
        .iter()
        .filter(|f| f.typ == "batch")
        .collect();
    assert!(!batches.is_empty(), "no batch fixtures found");

    for fix in batches {
        let raw = from_hex(&fix.hex);
        let decoded = decode_frame(&raw).unwrap_or_else(|e| {
            panic!("[{}] decode failed: {e}", fix.label);
        });

        let expected_items = fix.decoded["items"].as_array().unwrap();

        match decoded {
            DecodedFrame::Batch(items) => {
                assert_eq!(
                    items.len(),
                    expected_items.len(),
                    "[{}] batch item count mismatch",
                    fix.label
                );
                for (i, (actual, expected)) in
                    items.iter().zip(expected_items.iter()).enumerate()
                {
                    let exp_channel = expected["channel_id"].as_u64().unwrap();
                    let exp_data = from_hex(expected["inner_hex"].as_str().unwrap());
                    assert_eq!(
                        actual.channel_id, exp_channel,
                        "[{}] batch item {i} channel_id mismatch",
                        fix.label
                    );
                    assert_eq!(
                        actual.data, exp_data,
                        "[{}] batch item {i} data mismatch",
                        fix.label
                    );
                }
            }
            other => panic!("[{}] expected Batch, got {other:?}", fix.label),
        }
    }
}

#[test]
fn golden_protomux_encode_roundtrip() {
    let file = load_fixtures();

    for fix in &file.fixtures.frames {
        let expected_bytes = from_hex(&fix.hex);
        let d = &fix.decoded;

        let encoded = match fix.typ.as_str() {
            "open" => {
                let local_id = d["local_id"].as_u64().unwrap();
                let protocol = d["protocol"].as_str().unwrap();
                let id = opt_hex(&d["id"]);
                let handshake = opt_hex(&d["handshake"]);
                encode_open(local_id, protocol, id.as_deref(), handshake.as_deref())
            }
            "close" => {
                let local_id = d["local_id"].as_u64().unwrap();
                encode_close(local_id)
            }
            "reject" => {
                let remote_id = d["remote_id"].as_u64().unwrap();
                encode_reject(remote_id)
            }
            "message" => {
                let channel_id = d["channel_id"].as_u64().unwrap();
                let message_type = d["message_type"].as_u64().unwrap();
                let payload = from_hex(d["payload"].as_str().unwrap());
                encode_message(channel_id, message_type, &payload)
            }
            "batch" => {
                let items = d["items"].as_array().unwrap();
                let entries: Vec<(u64, Vec<u8>)> = items
                    .iter()
                    .map(|item| {
                        let channel_id = item["channel_id"].as_u64().unwrap();
                        let data = from_hex(item["inner_hex"].as_str().unwrap());
                        (channel_id, data)
                    })
                    .collect();
                encode_batch(&entries)
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

#[test]
fn golden_protomux_conversation_decode() {
    let file = load_fixtures();

    for conv in &file.fixtures.conversations {
        for (direction, frames) in [("a_to_b", &conv.a_to_b), ("b_to_a", &conv.b_to_a)] {
            for (i, frame_hex) in frames.iter().enumerate() {
                let raw = from_hex(frame_hex);
                let decoded = decode_frame(&raw).unwrap_or_else(|e| {
                    panic!(
                        "[{}] {direction} frame {i} decode failed: {e}\n  hex: {frame_hex}",
                        conv.label
                    );
                });

                match &decoded {
                    DecodedFrame::Control(ctrl) => match ctrl {
                        ControlFrame::Open { protocol, .. } => {
                            assert_eq!(
                                protocol, &conv.protocol,
                                "[{}] {direction} frame {i}: expected protocol '{}'",
                                conv.label, conv.protocol,
                            );
                        }
                        ControlFrame::Close { .. } | ControlFrame::Reject { .. } => {}
                    },
                    DecodedFrame::Message { channel_id, .. } => {
                        assert!(
                            *channel_id > 0,
                            "[{}] {direction} frame {i}: data message has channel_id 0",
                            conv.label,
                        );
                    }
                    DecodedFrame::Batch(items) => {
                        for item in items {
                            assert!(
                                item.channel_id > 0,
                                "[{}] {direction} frame {i}: batch item has channel_id 0",
                                conv.label,
                            );
                        }
                    }
                }
            }
        }
    }
}
