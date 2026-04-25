use peeroxide_dht::hyperdht_messages::HolepunchPayload;
use peeroxide_dht::messages::Ipv4Peer;
use peeroxide_dht::secure_payload::SecurePayload;
use serde::Deserialize;

#[derive(Deserialize)]
struct FixtureFile {
    key_hex: String,
    local_secret_hex: String,
    fixtures: Vec<Fixture>,
    tokens: std::collections::HashMap<String, String>,
}

#[derive(Deserialize)]
struct Fixture {
    label: String,
    nonce_hex: String,
    encrypted_hex: String,
    payload: PayloadFields,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PayloadFields {
    error: u64,
    firewall: u64,
    round: u64,
    connected: bool,
    punching: bool,
    addresses: Option<Vec<AddrFields>>,
    remote_address: Option<AddrFields>,
    token: Option<String>,
    remote_token: Option<String>,
}

#[derive(Deserialize)]
struct AddrFields {
    host: String,
    port: u16,
}

fn load_fixtures() -> FixtureFile {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tests/interop/secure-payload-fixtures.json"
    );
    let data = std::fs::read_to_string(path).unwrap_or_else(|e| {
        panic!("Failed to read secure-payload fixtures at {path}: {e}.")
    });
    serde_json::from_str(&data)
        .unwrap_or_else(|e| panic!("Failed to parse secure-payload fixtures: {e}"))
}

fn hex_bytes(hex_str: &str) -> Vec<u8> {
    hex::decode(hex_str).unwrap_or_else(|e| panic!("Invalid hex '{hex_str}': {e}"))
}

fn hex_array_32(hex_str: &str) -> [u8; 32] {
    let b = hex_bytes(hex_str);
    b.try_into().expect("expected 32 bytes")
}

fn hex_array_24(hex_str: &str) -> [u8; 24] {
    let b = hex_bytes(hex_str);
    b.try_into().expect("expected 24 bytes")
}

fn payload_from_fields(f: &PayloadFields) -> HolepunchPayload {
    HolepunchPayload {
        error: f.error,
        firewall: f.firewall,
        round: f.round,
        connected: f.connected,
        punching: f.punching,
        addresses: f.addresses.as_ref().map(|addrs| {
            addrs
                .iter()
                .map(|a| Ipv4Peer {
                    host: a.host.clone(),
                    port: a.port,
                })
                .collect()
        }),
        remote_address: f.remote_address.as_ref().map(|a| Ipv4Peer {
            host: a.host.clone(),
            port: a.port,
        }),
        token: f.token.as_ref().map(|h| hex_array_32(h)),
        remote_token: f.remote_token.as_ref().map(|h| hex_array_32(h)),
    }
}

/// Verify that Rust can decrypt ciphertext produced by Node.js SecurePayload.
#[test]
fn golden_secure_payload_decrypt() {
    let file = load_fixtures();
    let key = hex_array_32(&file.key_hex);
    let sp = SecurePayload::new(key);

    for fixture in &file.fixtures {
        let encrypted = hex_bytes(&fixture.encrypted_hex);
        let expected = payload_from_fields(&fixture.payload);

        let decrypted = sp.decrypt(&encrypted).unwrap_or_else(|e| {
            panic!("DECRYPT {} failed: {e}", fixture.label);
        });
        assert_eq!(
            decrypted, expected,
            "DECRYPT mismatch for {}",
            fixture.label
        );
    }
}

/// Verify that Rust encryption with the same nonce produces byte-identical ciphertext.
#[test]
fn golden_secure_payload_encrypt() {
    let file = load_fixtures();
    let key = hex_array_32(&file.key_hex);
    let sp = SecurePayload::new(key);

    for fixture in &file.fixtures {
        let expected_encrypted = hex_bytes(&fixture.encrypted_hex);
        let nonce = hex_array_24(&fixture.nonce_hex);
        let payload = payload_from_fields(&fixture.payload);

        let encrypted = sp.encrypt_with_nonce(&payload, nonce).unwrap_or_else(|e| {
            panic!("ENCRYPT {} failed: {e}", fixture.label);
        });
        assert_eq!(
            hex::encode(&encrypted),
            hex::encode(&expected_encrypted),
            "ENCRYPT mismatch for {}: Rust={} Node={}",
            fixture.label,
            hex::encode(&encrypted),
            hex::encode(&expected_encrypted)
        );
    }
}

/// Verify that Rust token generation matches Node.js crypto_generichash output.
#[test]
fn golden_secure_payload_tokens() {
    let file = load_fixtures();
    let key = hex_array_32(&file.key_hex);
    let local_secret = hex_array_32(&file.local_secret_hex);
    let sp = SecurePayload::with_local_secret(key, local_secret);

    for (host, expected_hex) in &file.tokens {
        let token = sp.token(host);
        assert_eq!(
            hex::encode(token),
            *expected_hex,
            "TOKEN mismatch for host {host}: Rust={} Node={expected_hex}",
            hex::encode(token)
        );
    }
}
