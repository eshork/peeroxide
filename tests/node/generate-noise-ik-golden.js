'use strict';

const NoiseHandshake = require('noise-handshake');
const NoiseWrap = require('hyperdht/lib/noise-wrap');
const curve = require('noise-curve-ed');
const c = require('compact-encoding');
const m = require('hyperdht/lib/messages');
const { NS } = require('hyperdht/lib/constants');
const sodium = require('sodium-universal');
const fs = require('fs');
const path = require('path');

const fixtures = [];

function hex(buf) {
  return Buffer.from(buf).toString('hex');
}

function seedKeypair(seedByte) {
  const seed = Buffer.alloc(32, seedByte);
  return curve.generateKeyPair(seed);
}

function copyBuf(buf) {
  const out = Buffer.allocUnsafe(buf.length);
  buf.copy(out);
  return out;
}

// ── Raw Noise IK handshake (no payload wrapping) ────────────────────────────

{
  const staticI = seedKeypair(0x50);
  const staticR = seedKeypair(0x60);
  const ephI = seedKeypair(0x70);
  const ephR = seedKeypair(0x80);
  const ephI_pk = copyBuf(ephI.publicKey);
  const ephR_pk = copyBuf(ephR.publicKey);

  const initiator = new NoiseHandshake('IK', true, staticI, { curve });
  const responder = new NoiseHandshake('IK', false, staticR, { curve });

  initiator.initialise(NS.PEER_HANDSHAKE, staticR.publicKey);
  responder.initialise(NS.PEER_HANDSHAKE);

  initiator.e = ephI;
  const m1_hex = hex(initiator.send());
  const m1_len = m1_hex.length / 2;
  responder.recv(Buffer.from(m1_hex, 'hex'));

  responder.e = ephR;
  const m2_hex = hex(responder.send());
  const m2_len = m2_hex.length / 2;
  initiator.recv(Buffer.from(m2_hex, 'hex'));

  if (!initiator.complete || !responder.complete) {
    throw new Error('IK handshake did not complete');
  }
  if (!Buffer.from(initiator.tx).equals(Buffer.from(responder.rx))) {
    throw new Error('IK tx/rx mismatch');
  }
  if (!Buffer.from(initiator.hash).equals(Buffer.from(responder.hash))) {
    throw new Error('IK hash mismatch');
  }

  fixtures.push({
    type: 'noise_ik_handshake',
    label: 'deterministic_ik_empty_payload',
    static_initiator_seed: hex(Buffer.alloc(32, 0x50)),
    static_initiator_pk: hex(staticI.publicKey),
    static_initiator_sk: hex(staticI.secretKey),
    static_responder_seed: hex(Buffer.alloc(32, 0x60)),
    static_responder_pk: hex(staticR.publicKey),
    static_responder_sk: hex(staticR.secretKey),
    ephemeral_initiator_seed: hex(Buffer.alloc(32, 0x70)),
    ephemeral_initiator_pk: hex(ephI_pk),
    ephemeral_responder_seed: hex(Buffer.alloc(32, 0x80)),
    ephemeral_responder_pk: hex(ephR_pk),
    prologue: hex(NS.PEER_HANDSHAKE),
    message1: m1_hex,
    message1_len: m1_len,
    message2: m2_hex,
    message2_len: m2_len,
    initiator_tx: hex(initiator.tx),
    initiator_rx: hex(initiator.rx),
    responder_tx: hex(responder.tx),
    responder_rx: hex(responder.rx),
    handshake_hash: hex(initiator.hash),
  });
}

// ── Raw Noise IK with payload ───────────────────────────────────────────────

{
  const staticI = seedKeypair(0xa1);
  const staticR = seedKeypair(0xa2);
  const ephI = seedKeypair(0xa3);
  const ephR = seedKeypair(0xa4);
  const ephI_pk = copyBuf(ephI.publicKey);
  const ephR_pk = copyBuf(ephR.publicKey);

  const initiator = new NoiseHandshake('IK', true, staticI, { curve });
  const responder = new NoiseHandshake('IK', false, staticR, { curve });

  initiator.initialise(NS.PEER_HANDSHAKE, staticR.publicKey);
  responder.initialise(NS.PEER_HANDSHAKE);

  const payloadI = Buffer.from('initiator-hello');
  const payloadR = Buffer.from('responder-hello');

  initiator.e = ephI;
  const m1_hex = hex(initiator.send(payloadI));
  const m1_len = m1_hex.length / 2;
  const recvPayloadI = Buffer.from(responder.recv(Buffer.from(m1_hex, 'hex')));

  responder.e = ephR;
  const m2_hex = hex(responder.send(payloadR));
  const m2_len = m2_hex.length / 2;
  const recvPayloadR = Buffer.from(initiator.recv(Buffer.from(m2_hex, 'hex')));

  if (!recvPayloadI.equals(payloadI)) throw new Error('payload mismatch M1');
  if (!recvPayloadR.equals(payloadR)) throw new Error('payload mismatch M2');

  fixtures.push({
    type: 'noise_ik_handshake',
    label: 'deterministic_ik_with_payload',
    static_initiator_seed: hex(Buffer.alloc(32, 0xa1)),
    static_initiator_pk: hex(staticI.publicKey),
    static_initiator_sk: hex(staticI.secretKey),
    static_responder_seed: hex(Buffer.alloc(32, 0xa2)),
    static_responder_pk: hex(staticR.publicKey),
    static_responder_sk: hex(staticR.secretKey),
    ephemeral_initiator_seed: hex(Buffer.alloc(32, 0xa3)),
    ephemeral_initiator_pk: hex(ephI_pk),
    ephemeral_responder_seed: hex(Buffer.alloc(32, 0xa4)),
    ephemeral_responder_pk: hex(ephR_pk),
    prologue: hex(NS.PEER_HANDSHAKE),
    payload_initiator: hex(payloadI),
    payload_responder: hex(payloadR),
    message1: m1_hex,
    message1_len: m1_len,
    message2: m2_hex,
    message2_len: m2_len,
    initiator_tx: hex(initiator.tx),
    initiator_rx: hex(initiator.rx),
    responder_tx: hex(responder.tx),
    responder_rx: hex(responder.rx),
    handshake_hash: hex(initiator.hash),
  });
}

// ── NoiseWrap (IK + NoisePayload encoding + holepunch secret) ───────────────

{
  const staticI = seedKeypair(0xb1);
  const staticR = seedKeypair(0xb2);
  const ephI = seedKeypair(0xb3);
  const ephR = seedKeypair(0xb4);
  const ephI_pk = copyBuf(ephI.publicKey);
  const ephR_pk = copyBuf(ephR.publicKey);

  const wrapI = new NoiseWrap(staticI, staticR.publicKey);
  const wrapR = new NoiseWrap(staticR);

  wrapI.handshake.e = ephI;

  const payloadI = {
    version: 1,
    error: 0,
    firewall: 2,
    holepunch: null,
    addresses4: [{ host: '192.168.1.10', port: 9000 }],
    addresses6: [],
    udx: { version: 1, reusableSocket: false, id: 1, seq: 0 },
    secretStream: { version: 1 },
    relayThrough: null,
    relayAddresses: null,
  };

  const m1_hex = hex(wrapI.send(payloadI));
  const m1_len = m1_hex.length / 2;
  wrapR.recv(Buffer.from(m1_hex, 'hex'));

  wrapR.handshake.e = ephR;

  const payloadR = {
    version: 1,
    error: 0,
    firewall: 1,
    holepunch: { id: 42, relays: [] },
    addresses4: [{ host: '10.0.0.1', port: 8080 }],
    addresses6: [],
    udx: { version: 1, reusableSocket: true, id: 7, seq: 3 },
    secretStream: { version: 1 },
    relayThrough: null,
    relayAddresses: null,
  };

  const m2_hex = hex(wrapR.send(payloadR));
  const m2_len = m2_hex.length / 2;
  wrapI.recv(Buffer.from(m2_hex, 'hex'));

  const resultI = wrapI.final();
  const resultR = wrapR.final();

  if (!resultI.holepunchSecret.equals(resultR.holepunchSecret)) {
    throw new Error('holepunch secret mismatch');
  }

  const encodedPayloadI = c.encode(m.noisePayload, payloadI);
  const encodedPayloadR = c.encode(m.noisePayload, payloadR);

  fixtures.push({
    type: 'noise_wrap',
    label: 'full_noisewrap_handshake',
    static_initiator_seed: hex(Buffer.alloc(32, 0xb1)),
    static_initiator_pk: hex(staticI.publicKey),
    static_initiator_sk: hex(staticI.secretKey),
    static_responder_seed: hex(Buffer.alloc(32, 0xb2)),
    static_responder_pk: hex(staticR.publicKey),
    static_responder_sk: hex(staticR.secretKey),
    ephemeral_initiator_seed: hex(Buffer.alloc(32, 0xb3)),
    ephemeral_initiator_pk: hex(ephI_pk),
    ephemeral_responder_seed: hex(Buffer.alloc(32, 0xb4)),
    ephemeral_responder_pk: hex(ephR_pk),
    payload_initiator_encoded: hex(encodedPayloadI),
    payload_responder_encoded: hex(encodedPayloadR),
    payload_initiator: {
      firewall: payloadI.firewall,
      addresses4: payloadI.addresses4,
      udx_id: payloadI.udx.id,
    },
    payload_responder: {
      firewall: payloadR.firewall,
      holepunch_id: payloadR.holepunch.id,
      addresses4: payloadR.addresses4,
      udx_id: payloadR.udx.id,
      udx_seq: payloadR.udx.seq,
      udx_reusable: payloadR.udx.reusableSocket,
    },
    message1: m1_hex,
    message1_len: m1_len,
    message2: m2_hex,
    message2_len: m2_len,
    initiator_tx: hex(resultI.tx),
    initiator_rx: hex(resultI.rx),
    responder_tx: hex(resultR.tx),
    responder_rx: hex(resultR.rx),
    handshake_hash: hex(resultI.hash),
    holepunch_secret: hex(resultI.holepunchSecret),
  });
}

// ── Holepunch secret derivation only ────────────────────────────────────────

{
  const testHash = Buffer.alloc(64, 0x55);
  const secret = Buffer.allocUnsafe(32);
  sodium.crypto_generichash(secret, NS.PEER_HOLEPUNCH, testHash);

  fixtures.push({
    type: 'holepunch_secret_derivation',
    label: 'fixed_hash_0x55',
    handshake_hash: hex(testHash),
    ns_peer_holepunch: hex(NS.PEER_HOLEPUNCH),
    holepunch_secret: hex(secret),
  });

  const testHash2 = Buffer.alloc(64);
  for (let i = 0; i < 64; i++) testHash2[i] = i;
  const secret2 = Buffer.allocUnsafe(32);
  sodium.crypto_generichash(secret2, NS.PEER_HOLEPUNCH, testHash2);

  fixtures.push({
    type: 'holepunch_secret_derivation',
    label: 'sequential_hash',
    handshake_hash: hex(testHash2),
    ns_peer_holepunch: hex(NS.PEER_HOLEPUNCH),
    holepunch_secret: hex(secret2),
  });
}

// ── Write output ────────────────────────────────────────────────────────────

const outPath = path.join(__dirname, '..', 'interop', 'noise-ik-fixtures.json');
fs.writeFileSync(outPath, JSON.stringify(fixtures, null, 2) + '\n');
console.log(`Wrote ${fixtures.length} fixtures to ${outPath}`);
