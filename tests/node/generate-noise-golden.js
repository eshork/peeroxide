'use strict';

const sodium = require('sodium-universal');
const ed = require('noise-curve-ed');
const Noise = require('noise-handshake');
const { Push, Pull, KEYBYTES, HEADERBYTES, ABYTES } = require('sodium-secretstream');
const fs = require('fs');
const path = require('path');

const fixtures = [];

// ── Helpers ─────────────────────────────────────────────────────────────────

function hex(buf) {
  return Buffer.from(buf).toString('hex');
}

function seedKeypair(seedByte) {
  const seed = Buffer.alloc(32, seedByte);
  return ed.generateKeyPair(seed);
}

// ── Ed25519 DH fixtures ────────────────────────────────────────────────────

// Generate deterministic keypairs from known seeds and compute DH
const kpA = seedKeypair(0x01);
const kpB = seedKeypair(0x02);

// DH(A_secret, B_public) should equal DH(B_secret, A_public)
const dhAB = ed.dh(kpB.publicKey, kpA);  // dh(remote_public, local_keypair)
const dhBA = ed.dh(kpA.publicKey, kpB);

if (!Buffer.from(dhAB).equals(Buffer.from(dhBA))) {
  throw new Error('DH symmetry check failed!');
}

fixtures.push({
  type: 'ed25519_dh',
  label: 'dh_symmetric',
  seed_a: hex(Buffer.alloc(32, 0x01)),
  public_key_a: hex(kpA.publicKey),
  secret_key_a: hex(kpA.secretKey),
  seed_b: hex(Buffer.alloc(32, 0x02)),
  public_key_b: hex(kpB.publicKey),
  secret_key_b: hex(kpB.secretKey),
  dh_output: hex(dhAB),
});

// Additional DH test with different seeds
const kpC = seedKeypair(0xaa);
const kpD = seedKeypair(0xff);
const dhCD = ed.dh(kpD.publicKey, kpC);

fixtures.push({
  type: 'ed25519_dh',
  label: 'dh_varied_seeds',
  seed_a: hex(Buffer.alloc(32, 0xaa)),
  public_key_a: hex(kpC.publicKey),
  secret_key_a: hex(kpC.secretKey),
  seed_b: hex(Buffer.alloc(32, 0xff)),
  public_key_b: hex(kpD.publicKey),
  secret_key_b: hex(kpD.secretKey),
  dh_output: hex(dhCD),
});

// ── Noise XX handshake fixtures ─────────────────────────────────────────────

// Deterministic seeds for static and ephemeral keys
const staticI = seedKeypair(0x10);
const staticR = seedKeypair(0x20);
const ephI = seedKeypair(0x30);
const ephR = seedKeypair(0x40);
const ephI_pk_copy = Buffer.from(ephI.publicKey);
const ephR_pk_copy = Buffer.from(ephR.publicKey);

const initiator = new Noise('XX', true, staticI, { curve: ed });
const responder = new Noise('XX', false, staticR, { curve: ed });

initiator.initialise(Buffer.alloc(0));
responder.initialise(Buffer.alloc(0));

// M1: initiator → responder (TOK_E)
// Pre-set deterministic ephemeral before send
initiator.e = ephI;
const m1_raw = initiator.send();
const m1 = Buffer.from(m1_raw);

responder.recv(m1_raw);

responder.e = ephR;
const m2_raw = responder.send();
const m2 = Buffer.from(m2_raw);

initiator.recv(m2_raw);

const m3_raw = initiator.send();
const m3 = Buffer.from(m3_raw);

responder.recv(m3_raw);

// Verify both sides complete
if (!initiator.complete || !responder.complete) {
  throw new Error('Handshake did not complete!');
}

// Verify key agreement
if (!Buffer.from(initiator.tx).equals(Buffer.from(responder.rx))) {
  throw new Error('tx/rx key mismatch (initiator.tx != responder.rx)');
}
if (!Buffer.from(initiator.rx).equals(Buffer.from(responder.tx))) {
  throw new Error('tx/rx key mismatch (initiator.rx != responder.tx)');
}
if (!Buffer.from(initiator.hash).equals(Buffer.from(responder.hash))) {
  throw new Error('handshake hash mismatch');
}

fixtures.push({
  type: 'noise_xx_handshake',
  label: 'deterministic_xx',
  static_initiator_seed: hex(Buffer.alloc(32, 0x10)),
  static_initiator_pk: hex(staticI.publicKey),
  static_initiator_sk: hex(staticI.secretKey),
  static_responder_seed: hex(Buffer.alloc(32, 0x20)),
  static_responder_pk: hex(staticR.publicKey),
  static_responder_sk: hex(staticR.secretKey),
  ephemeral_initiator_seed: hex(Buffer.alloc(32, 0x30)),
  ephemeral_initiator_pk: hex(ephI_pk_copy),
  ephemeral_responder_seed: hex(Buffer.alloc(32, 0x40)),
  ephemeral_responder_pk: hex(ephR_pk_copy),
  message1: hex(m1),
  message1_len: m1.byteLength,
  message2: hex(m2),
  message2_len: m2.byteLength,
  message3: hex(m3),
  message3_len: m3.byteLength,
  initiator_tx: hex(initiator.tx),
  initiator_rx: hex(initiator.rx),
  responder_tx: hex(responder.tx),
  responder_rx: hex(responder.rx),
  handshake_hash: hex(initiator.hash),
});

// ── Secretstream fixtures ───────────────────────────────────────────────────

// Use the Noise-derived keys for a realistic secretstream test.
// Push with initiator.tx, Pull with responder.rx (which should be equal).
const ssKey = Buffer.from(initiator.tx);
const push = new Push(ssKey);
const header = Buffer.from(push.header);

// Encrypt several messages
const msg1 = Buffer.from('hello');
const msg2 = Buffer.from('world');
const msg3 = Buffer.from('');  // empty message
const msg4 = Buffer.from('final message');

const ct1 = push.next(msg1);
const ct2 = push.next(msg2);
const ct3 = push.next(msg3);
const ct4 = push.final(msg4, Buffer.allocUnsafe(msg4.byteLength + ABYTES));

// Verify decryption works on JS side
const pull = new Pull(Buffer.from(ssKey));
pull.init(header);
const dec1 = pull.next(Buffer.from(ct1));
const dec2 = pull.next(Buffer.from(ct2));
const dec3 = pull.next(Buffer.from(ct3));
const dec4 = pull.next(Buffer.from(ct4));

if (!dec1.equals(msg1)) throw new Error('Secretstream roundtrip failed: msg1');
if (!dec2.equals(msg2)) throw new Error('Secretstream roundtrip failed: msg2');
if (!dec3.equals(msg3)) throw new Error('Secretstream roundtrip failed: msg3');
if (!dec4.equals(msg4)) throw new Error('Secretstream roundtrip failed: msg4');
if (!pull.final) throw new Error('Secretstream did not detect TAG_FINAL');

fixtures.push({
  type: 'secretstream',
  label: 'multi_message',
  key: hex(ssKey),
  header: hex(header),
  messages: [
    { plaintext: hex(msg1), ciphertext: hex(ct1), tag: 'message', plaintext_str: 'hello' },
    { plaintext: hex(msg2), ciphertext: hex(ct2), tag: 'message', plaintext_str: 'world' },
    { plaintext: hex(msg3), ciphertext: hex(ct3), tag: 'message', plaintext_str: '' },
    { plaintext: hex(msg4), ciphertext: hex(ct4), tag: 'final', plaintext_str: 'final message' },
  ],
  abytes: ABYTES,
  headerbytes: HEADERBYTES,
  keybytes: KEYBYTES,
});

// Second secretstream test with a standalone key (not from Noise)
const ssKey2 = Buffer.alloc(32);
sodium.crypto_secretstream_xchacha20poly1305_keygen(ssKey2);
const push2 = new Push(ssKey2);
const header2 = Buffer.from(push2.header);

const bigMsg = Buffer.alloc(1024, 0x42);
const ctBig = push2.next(bigMsg);
const ctFinal2 = push2.final();

const pull2 = new Pull(Buffer.from(ssKey2));
pull2.init(header2);
const decBig = pull2.next(Buffer.from(ctBig));
const decFinal2 = pull2.next(Buffer.from(ctFinal2));

if (!decBig.equals(bigMsg)) throw new Error('Big msg roundtrip failed');
if (!pull2.final) throw new Error('TAG_FINAL not detected');

fixtures.push({
  type: 'secretstream',
  label: 'big_message_and_empty_final',
  key: hex(ssKey2),
  header: hex(header2),
  messages: [
    { plaintext: hex(bigMsg), ciphertext: hex(ctBig), tag: 'message', plaintext_len: 1024 },
    { plaintext: '', ciphertext: hex(ctFinal2), tag: 'final', plaintext_len: 0 },
  ],
  abytes: ABYTES,
  headerbytes: HEADERBYTES,
  keybytes: KEYBYTES,
});

// ── Write output ────────────────────────────────────────────────────────────

const outPath = path.join(__dirname, '..', 'interop', 'noise-fixtures.json');
fs.writeFileSync(outPath, JSON.stringify(fixtures, null, 2) + '\n');
console.log(`Wrote ${fixtures.length} fixtures to ${outPath}`);
