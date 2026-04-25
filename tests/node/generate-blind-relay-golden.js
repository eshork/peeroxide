'use strict';

const c = require('compact-encoding');
const bitfield = require('compact-encoding-bitfield');
const bits = require('bits-to-bytes');
const fs = require('fs');
const path = require('path');

const blindRelayPkg = require('./node_modules/blind-relay/package.json');
const flags = bitfield(7);

const fixtures = [];

function encodePair(isInitiator, token, id, seq) {
  const state = { start: 0, end: 0, buffer: null };
  flags.preencode(state);
  c.fixed32.preencode(state, token);
  c.uint.preencode(state, id);
  c.uint.preencode(state, seq);
  state.buffer = Buffer.allocUnsafe(state.end);
  flags.encode(state, bits.of(isInitiator));
  c.fixed32.encode(state, token);
  c.uint.encode(state, id);
  c.uint.encode(state, seq);
  return state.buffer;
}

function encodeUnpair(token) {
  const state = { start: 0, end: 0, buffer: null };
  flags.preencode(state);
  c.fixed32.preencode(state, token);
  state.buffer = Buffer.allocUnsafe(state.end);
  flags.encode(state, bits.of());
  c.fixed32.encode(state, token);
  return state.buffer;
}

function add(label, type, hex, decoded) {
  fixtures.push({ label, type, hex, decoded });
}

const token_aa = Buffer.alloc(32, 0xaa);
const token_bb = Buffer.alloc(32, 0xbb);
const token_00 = Buffer.alloc(32, 0x00);
const token_ff = Buffer.alloc(32, 0xff);
const token_42 = Buffer.alloc(32, 0x42);

add('pair: initiator, small ids', 'pair',
  encodePair(true, token_42, 1, 2).toString('hex'),
  { is_initiator: true, token: token_42.toString('hex'), id: 1, seq: 2 }
);

add('pair: responder, zero ids', 'pair',
  encodePair(false, token_00, 0, 0).toString('hex'),
  { is_initiator: false, token: token_00.toString('hex'), id: 0, seq: 0 }
);

add('pair: initiator, large ids', 'pair',
  encodePair(true, token_aa, 100000, 65536).toString('hex'),
  { is_initiator: true, token: token_aa.toString('hex'), id: 100000, seq: 65536 }
);

add('pair: responder, all-ff token', 'pair',
  encodePair(false, token_ff, 42, 7).toString('hex'),
  { is_initiator: false, token: token_ff.toString('hex'), id: 42, seq: 7 }
);

add('pair: initiator, id=253 (multi-byte varint)', 'pair',
  encodePair(true, token_bb, 253, 253).toString('hex'),
  { is_initiator: true, token: token_bb.toString('hex'), id: 253, seq: 253 }
);

add('unpair: all-aa token', 'unpair',
  encodeUnpair(token_aa).toString('hex'),
  { token: token_aa.toString('hex') }
);

add('unpair: zero token', 'unpair',
  encodeUnpair(token_00).toString('hex'),
  { token: token_00.toString('hex') }
);

add('unpair: all-ff token', 'unpair',
  encodeUnpair(token_ff).toString('hex'),
  { token: token_ff.toString('hex') }
);

const output = {
  generated_by: 'blind-relay golden fixture generator',
  blind_relay_version: blindRelayPkg.version,
  fixtures,
};

const outDir = path.resolve(__dirname, '..', 'interop');
fs.mkdirSync(outDir, { recursive: true });
const outPath = path.join(outDir, 'blind-relay-fixtures.json');
fs.writeFileSync(outPath, JSON.stringify(output, null, 2), 'utf8');

console.log(`Generated ${fixtures.length} fixtures → ${outPath}`);
