'use strict';

const c = require('compact-encoding');
const m = require('hyperdht/lib/messages');
const { NS } = require('hyperdht/lib/constants');
const hdrPkg = require('hyperdht/package.json');
const fs = require('fs');
const path = require('path');

const fixtures = [];

function encodeHex(enc, val) {
  return c.encode(enc, val).toString('hex');
}

function peerFields(peer) {
  return {
    publicKey: peer.publicKey.toString('hex'),
    relayAddresses: peer.relayAddresses.map(a => ({ host: a.host, port: a.port })),
  };
}

// ── hyper_peer fixtures ──────────────────────────────────────────────────────

const peerNoRelay = { publicKey: Buffer.alloc(32, 0xaa), relayAddresses: [] };
fixtures.push({
  type: 'hyper_peer',
  label: 'peer_no_relay',
  fields: peerFields(peerNoRelay),
  hex: encodeHex(m.peer, peerNoRelay),
});

const peerWithRelay = {
  publicKey: Buffer.alloc(32, 0xbb),
  relayAddresses: [{ host: '10.0.0.1', port: 8080 }],
};
fixtures.push({
  type: 'hyper_peer',
  label: 'peer_with_relay',
  fields: peerFields(peerWithRelay),
  hex: encodeHex(m.peer, peerWithRelay),
});

const peerMultiRelay = {
  publicKey: Buffer.alloc(32, 0xcc),
  relayAddresses: [
    { host: '192.168.1.1', port: 3000 },
    { host: '10.0.0.2', port: 9090 },
  ],
};
fixtures.push({
  type: 'hyper_peer',
  label: 'peer_multi_relay',
  fields: peerFields(peerMultiRelay),
  hex: encodeHex(m.peer, peerMultiRelay),
});

// ── announce fixtures (all flag combinations) ────────────────────────────────

function announceFields(ann) {
  return {
    peer: ann.peer ? peerFields(ann.peer) : null,
    refresh: ann.refresh ? ann.refresh.toString('hex') : null,
    signature: ann.signature ? ann.signature.toString('hex') : null,
    bump: ann.bump || 0,
  };
}

const announceVariants = [
  {
    label: 'announce_empty',
    val: { peer: null, refresh: null, signature: null, bump: 0 },
  },
  {
    label: 'announce_peer_only',
    val: { peer: peerNoRelay, refresh: null, signature: null, bump: 0 },
  },
  {
    label: 'announce_peer_refresh',
    val: { peer: peerNoRelay, refresh: Buffer.alloc(32, 0x11), signature: null, bump: 0 },
  },
  {
    label: 'announce_peer_signature',
    val: { peer: peerNoRelay, refresh: null, signature: Buffer.alloc(64, 0x22), bump: 0 },
  },
  {
    label: 'announce_peer_bump',
    val: { peer: peerNoRelay, refresh: null, signature: null, bump: 5 },
  },
  {
    label: 'announce_all_flags',
    val: {
      peer: peerNoRelay,
      refresh: Buffer.alloc(32, 0x33),
      signature: Buffer.alloc(64, 0x44),
      bump: 99,
    },
  },
  {
    label: 'announce_bump_only',
    val: { peer: null, refresh: null, signature: null, bump: 42 },
  },
];

for (const { label, val } of announceVariants) {
  fixtures.push({
    type: 'announce',
    label,
    fields: announceFields(val),
    hex: encodeHex(m.announce, val),
  });
}

// ── lookupRawReply fixtures ──────────────────────────────────────────────────

function lookupReplyFields(reply) {
  return {
    peers: reply.peers.map(buf => {
      const decoded = c.decode(m.peer, buf);
      return peerFields(decoded);
    }),
    bump: reply.bump,
  };
}

const lrr1 = { peers: [c.encode(m.peer, peerNoRelay)], bump: 0 };
fixtures.push({
  type: 'lookup_raw_reply',
  label: 'lookup_raw_reply_one_peer',
  fields: lookupReplyFields(lrr1),
  hex: encodeHex(m.lookupRawReply, lrr1),
});

const lrr2 = {
  peers: [c.encode(m.peer, peerNoRelay), c.encode(m.peer, peerWithRelay)],
  bump: 7,
};
fixtures.push({
  type: 'lookup_raw_reply',
  label: 'lookup_raw_reply_two_peers_bump',
  fields: lookupReplyFields(lrr2),
  hex: encodeHex(m.lookupRawReply, lrr2),
});

const lrr0 = { peers: [], bump: 0 };
fixtures.push({
  type: 'lookup_raw_reply',
  label: 'lookup_raw_reply_empty',
  fields: lookupReplyFields(lrr0),
  hex: encodeHex(m.lookupRawReply, lrr0),
});

// ── mutablePutRequest fixtures ───────────────────────────────────────────────

const mpr = {
  publicKey: Buffer.alloc(32, 0xde),
  seq: 3,
  value: Buffer.from('hello world'),
  signature: Buffer.alloc(64, 0xef),
};
fixtures.push({
  type: 'mutable_put_request',
  label: 'mutable_put_request_basic',
  fields: {
    publicKey: mpr.publicKey.toString('hex'),
    seq: mpr.seq,
    value: mpr.value.toString('hex'),
    signature: mpr.signature.toString('hex'),
  },
  hex: encodeHex(m.mutablePutRequest, mpr),
});

const mprHighSeq = {
  publicKey: Buffer.alloc(32, 0x01),
  seq: 65536,
  value: Buffer.from([1, 2, 3]),
  signature: Buffer.alloc(64, 0x99),
};
fixtures.push({
  type: 'mutable_put_request',
  label: 'mutable_put_request_high_seq',
  fields: {
    publicKey: mprHighSeq.publicKey.toString('hex'),
    seq: mprHighSeq.seq,
    value: mprHighSeq.value.toString('hex'),
    signature: mprHighSeq.signature.toString('hex'),
  },
  hex: encodeHex(m.mutablePutRequest, mprHighSeq),
});

// ── mutableGetResponse fixtures ──────────────────────────────────────────────

const mgr = {
  seq: 5,
  value: Buffer.from('response value'),
  signature: Buffer.alloc(64, 0x77),
};
fixtures.push({
  type: 'mutable_get_response',
  label: 'mutable_get_response_basic',
  fields: {
    seq: mgr.seq,
    value: mgr.value.toString('hex'),
    signature: mgr.signature.toString('hex'),
  },
  hex: encodeHex(m.mutableGetResponse, mgr),
});

// ── mutableSignable fixtures ─────────────────────────────────────────────────

const ms = { seq: 10, value: Buffer.from('signable data') };
fixtures.push({
  type: 'mutable_signable',
  label: 'mutable_signable_basic',
  fields: {
    seq: ms.seq,
    value: ms.value.toString('hex'),
  },
  hex: encodeHex(m.mutableSignable, ms),
});

const msZero = { seq: 0, value: Buffer.from([0xff]) };
fixtures.push({
  type: 'mutable_signable',
  label: 'mutable_signable_seq_zero',
  fields: {
    seq: msZero.seq,
    value: msZero.value.toString('hex'),
  },
  hex: encodeHex(m.mutableSignable, msZero),
});

// ── namespace derivation fixtures ────────────────────────────────────────────

for (const [label, buf] of Object.entries(NS)) {
  fixtures.push({
    type: 'namespace',
    label: `NS_${label}`,
    hex: buf.toString('hex'),
  });
}

// ── handshake (PEER_HANDSHAKE) fixtures ──────────────────────────────────────

const handshakeVariants = [
  {
    label: 'handshake_no_addr',
    val: { mode: 0, noise: Buffer.alloc(48, 0xab), peerAddress: null, relayAddress: null },
  },
  {
    label: 'handshake_peer_addr',
    val: { mode: 1, noise: Buffer.alloc(32, 0xcd), peerAddress: { host: '192.168.1.1', port: 3000 }, relayAddress: null },
  },
  {
    label: 'handshake_relay_addr',
    val: { mode: 2, noise: Buffer.alloc(16, 0xef), peerAddress: null, relayAddress: { host: '10.0.0.1', port: 8080 } },
  },
  {
    label: 'handshake_both_addr',
    val: { mode: 4, noise: Buffer.alloc(64, 0x11), peerAddress: { host: '1.2.3.4', port: 1234 }, relayAddress: { host: '5.6.7.8', port: 5678 } },
  },
];

for (const { label, val } of handshakeVariants) {
  fixtures.push({
    type: 'handshake',
    label,
    fields: {
      mode: val.mode,
      noise: val.noise.toString('hex'),
      peerAddress: val.peerAddress,
      relayAddress: val.relayAddress,
    },
    hex: encodeHex(m.handshake, val),
  });
}

// ── holepunch (PEER_HOLEPUNCH) fixtures ──────────────────────────────────────

const holepunchVariants = [
  {
    label: 'holepunch_no_addr',
    val: { mode: 0, id: 42, payload: Buffer.alloc(24, 0xaa), peerAddress: null },
  },
  {
    label: 'holepunch_with_addr',
    val: { mode: 2, id: 9999, payload: Buffer.alloc(48, 0xbb), peerAddress: { host: '10.0.0.5', port: 4000 } },
  },
  {
    label: 'holepunch_empty_payload',
    val: { mode: 4, id: 0, payload: Buffer.alloc(0), peerAddress: null },
  },
];

for (const { label, val } of holepunchVariants) {
  fixtures.push({
    type: 'holepunch',
    label,
    fields: {
      mode: val.mode,
      id: val.id,
      payload: val.payload.toString('hex'),
      peerAddress: val.peerAddress,
    },
    hex: encodeHex(m.holepunch, val),
  });
}

// ── noisePayload fixtures ────────────────────────────────────────────────────

const noisePayloadVariants = [
  {
    label: 'noise_payload_minimal',
    val: {
      version: 1, error: 0, firewall: 0,
      holepunch: null, addresses4: [], addresses6: [],
      udx: null, secretStream: null, relayThrough: null, relayAddresses: null,
    },
  },
  {
    label: 'noise_payload_addresses4',
    val: {
      version: 1, error: 0, firewall: 1,
      holepunch: null,
      addresses4: [{ host: '1.2.3.4', port: 1000 }, { host: '5.6.7.8', port: 2000 }],
      addresses6: [],
      udx: null, secretStream: null, relayThrough: null, relayAddresses: null,
    },
  },
  {
    label: 'noise_payload_holepunch',
    val: {
      version: 1, error: 0, firewall: 2,
      holepunch: {
        id: 7,
        relays: [{ relayAddress: { host: '10.0.0.1', port: 8080 }, peerAddress: { host: '192.168.1.1', port: 3000 } }],
      },
      addresses4: [], addresses6: [],
      udx: null, secretStream: null, relayThrough: null, relayAddresses: null,
    },
  },
  {
    label: 'noise_payload_udx_ss',
    val: {
      version: 1, error: 0, firewall: 3,
      holepunch: null,
      addresses4: [{ host: '1.2.3.4', port: 1000 }],
      addresses6: [],
      udx: { version: 1, reusableSocket: true, id: 100, seq: 200 },
      secretStream: { version: 1 },
      relayThrough: null, relayAddresses: null,
    },
  },
  {
    label: 'noise_payload_all_fields',
    val: {
      version: 1, error: 1, firewall: 2,
      holepunch: { id: 42, relays: [] },
      addresses4: [{ host: '1.2.3.4', port: 1000 }],
      addresses6: [{ host: '2001:db8::1', port: 2000 }],
      udx: { version: 1, reusableSocket: false, id: 1, seq: 0 },
      secretStream: { version: 1 },
      relayThrough: { version: 1, publicKey: Buffer.alloc(32, 0xaa), token: Buffer.alloc(32, 0xbb) },
      relayAddresses: [{ host: '10.0.0.1', port: 8080 }],
    },
  },
  {
    label: 'noise_payload_relay_through',
    val: {
      version: 1, error: 0, firewall: 1,
      holepunch: null,
      addresses4: [], addresses6: [],
      udx: null, secretStream: null,
      relayThrough: { version: 1, publicKey: Buffer.alloc(32, 0xdd), token: Buffer.alloc(32, 0xee) },
      relayAddresses: null,
    },
  },
];

function noisePayloadFields(np) {
  const f = {
    version: np.version,
    error: np.error,
    firewall: np.firewall,
    addresses4: np.addresses4.map(a => ({ host: a.host, port: a.port })),
    addresses6: np.addresses6.map(a => ({ host: a.host, port: a.port })),
    holepunch: null,
    udx: null,
    secretStream: null,
    relayThrough: null,
    relayAddresses: null,
  };
  if (np.holepunch) {
    f.holepunch = {
      id: np.holepunch.id,
      relays: np.holepunch.relays.map(r => ({
        relayAddress: { host: r.relayAddress.host, port: r.relayAddress.port },
        peerAddress: { host: r.peerAddress.host, port: r.peerAddress.port },
      })),
    };
  }
  if (np.udx) {
    f.udx = { version: np.udx.version, reusableSocket: np.udx.reusableSocket, id: np.udx.id, seq: np.udx.seq };
  }
  if (np.secretStream) {
    f.secretStream = { version: np.secretStream.version };
  }
  if (np.relayThrough) {
    f.relayThrough = {
      version: np.relayThrough.version,
      publicKey: np.relayThrough.publicKey.toString('hex'),
      token: np.relayThrough.token.toString('hex'),
    };
  }
  if (np.relayAddresses) {
    f.relayAddresses = np.relayAddresses.map(a => ({ host: a.host, port: a.port }));
  }
  return f;
}

for (const { label, val } of noisePayloadVariants) {
  fixtures.push({
    type: 'noise_payload',
    label,
    fields: noisePayloadFields(val),
    hex: encodeHex(m.noisePayload, val),
  });
}

// ── holepunchPayload fixtures ────────────────────────────────────────────────

const holepunchPayloadVariants = [
  {
    label: 'holepunch_payload_minimal',
    val: {
      error: 0, firewall: 0, round: 0,
      connected: false, punching: false,
      addresses: null, remoteAddress: null, token: null, remoteToken: null,
    },
  },
  {
    label: 'holepunch_payload_connected_punching',
    val: {
      error: 0, firewall: 2, round: 3,
      connected: true, punching: true,
      addresses: null, remoteAddress: null, token: null, remoteToken: null,
    },
  },
  {
    label: 'holepunch_payload_with_addresses',
    val: {
      error: 0, firewall: 1, round: 1,
      connected: false, punching: true,
      addresses: [{ host: '1.2.3.4', port: 1000 }, { host: '5.6.7.8', port: 2000 }],
      remoteAddress: { host: '10.0.0.1', port: 8080 },
      token: null, remoteToken: null,
    },
  },
  {
    label: 'holepunch_payload_with_tokens',
    val: {
      error: 0, firewall: 3, round: 5,
      connected: false, punching: false,
      addresses: null, remoteAddress: null,
      token: Buffer.alloc(32, 0xaa), remoteToken: Buffer.alloc(32, 0xbb),
    },
  },
  {
    label: 'holepunch_payload_all_fields',
    val: {
      error: 1, firewall: 2, round: 10,
      connected: true, punching: true,
      addresses: [{ host: '192.168.1.1', port: 3000 }],
      remoteAddress: { host: '10.0.0.5', port: 4000 },
      token: Buffer.alloc(32, 0xcc), remoteToken: Buffer.alloc(32, 0xdd),
    },
  },
];

function holepunchPayloadFields(hp) {
  return {
    error: hp.error,
    firewall: hp.firewall,
    round: hp.round,
    connected: hp.connected,
    punching: hp.punching,
    addresses: hp.addresses ? hp.addresses.map(a => ({ host: a.host, port: a.port })) : null,
    remoteAddress: hp.remoteAddress ? { host: hp.remoteAddress.host, port: hp.remoteAddress.port } : null,
    token: hp.token ? hp.token.toString('hex') : null,
    remoteToken: hp.remoteToken ? hp.remoteToken.toString('hex') : null,
  };
}

for (const { label, val } of holepunchPayloadVariants) {
  fixtures.push({
    type: 'holepunch_payload',
    label,
    fields: holepunchPayloadFields(val),
    hex: encodeHex(m.holepunchPayload, val),
  });
}

// ── write output ─────────────────────────────────────────────────────────────

const outDir = path.resolve(__dirname, '../interop');
const outFile = path.join(outDir, 'hyperdht-fixtures.json');

fs.mkdirSync(outDir, { recursive: true });

const output = {
  generator: 'generate-hyperdht-golden.js',
  hyperdht_version: hdrPkg.version,
  fixtures,
};

fs.writeFileSync(outFile, JSON.stringify(output, null, 2) + '\n');
console.log(`Written ${fixtures.length} fixtures to ${outFile}`);
