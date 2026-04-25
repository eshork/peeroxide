'use strict';

const c = require('compact-encoding');
const fs = require('fs');
const path = require('path');
const Duplex = require('streamx').Duplex;

const pkgVersion = require('./node_modules/protomux/package.json').version;

const fixtures = { frames: [], conversations: [] };

// ── Part 1: Individual frame encoding ────────────────────────────────────────
// Manually construct protomux wire frames using compact-encoding.
// These match the exact byte output that protomux writes to the stream.

function encodeOpen(localId, protocol, id, handshake) {
  const state = { start: 0, end: 0, buffer: null };
  c.uint.preencode(state, 0);
  c.uint.preencode(state, 1);
  c.uint.preencode(state, localId);
  c.string.preencode(state, protocol);
  c.buffer.preencode(state, id);
  if (handshake) state.end += handshake.byteLength;
  state.buffer = Buffer.allocUnsafe(state.end);
  c.uint.encode(state, 0);
  c.uint.encode(state, 1);
  c.uint.encode(state, localId);
  c.string.encode(state, protocol);
  c.buffer.encode(state, id);
  if (handshake) handshake.copy(state.buffer, state.start);
  return state.buffer;
}

function encodeClose(localId) {
  const state = { start: 0, end: 0, buffer: null };
  c.uint.preencode(state, 0);
  c.uint.preencode(state, 3);
  c.uint.preencode(state, localId);
  state.buffer = Buffer.allocUnsafe(state.end);
  c.uint.encode(state, 0);
  c.uint.encode(state, 3);
  c.uint.encode(state, localId);
  return state.buffer;
}

function encodeReject(remoteId) {
  const state = { start: 0, end: 0, buffer: null };
  c.uint.preencode(state, 0);
  c.uint.preencode(state, 2);
  c.uint.preencode(state, remoteId);
  state.buffer = Buffer.allocUnsafe(state.end);
  c.uint.encode(state, 0);
  c.uint.encode(state, 2);
  c.uint.encode(state, remoteId);
  return state.buffer;
}

function encodeMessage(channelId, messageType, payload) {
  const state = { start: 0, end: 0, buffer: null };
  c.uint.preencode(state, channelId);
  c.uint.preencode(state, messageType);
  state.end += payload.byteLength;
  state.buffer = Buffer.allocUnsafe(state.end);
  c.uint.encode(state, channelId);
  c.uint.encode(state, messageType);
  payload.copy(state.buffer, state.start);
  return state.buffer;
}

function encodeBatch(entries) {
  // entries: [{channelId, data: Buffer}]
  if (entries.length === 0) return Buffer.alloc(0);

  const state = { start: 0, end: 0, buffer: null };
  c.uint.preencode(state, 0);
  c.uint.preencode(state, 0);
  c.uint.preencode(state, entries[0].channelId);

  let prevId = entries[0].channelId;
  for (const entry of entries) {
    if (entry.channelId !== prevId) {
      state.end += 1; // 0x00 separator
      c.uint.preencode(state, entry.channelId);
      prevId = entry.channelId;
    }
    c.buffer.preencode(state, entry.data);
  }

  state.buffer = Buffer.allocUnsafe(state.end);
  c.uint.encode(state, 0);
  c.uint.encode(state, 0);

  prevId = entries[0].channelId;
  c.uint.encode(state, prevId);

  for (const entry of entries) {
    if (entry.channelId !== prevId) {
      state.buffer[state.start++] = 0;
      c.uint.encode(state, entry.channelId);
      prevId = entry.channelId;
    }
    c.buffer.encode(state, entry.data);
  }

  return state.buffer;
}

function encodeBatchItem(messageType, payload) {
  const state = { start: 0, end: 0, buffer: null };
  c.uint.preencode(state, messageType);
  state.end += payload.byteLength;
  state.buffer = Buffer.allocUnsafe(state.end);
  c.uint.encode(state, messageType);
  payload.copy(state.buffer, state.start);
  return state.buffer;
}

function addFrame(label, type, frame, decoded) {
  fixtures.frames.push({
    label,
    type,
    hex: frame.toString('hex'),
    decoded,
  });
}

// Open frames
addFrame('open: basic channel', 'open',
  encodeOpen(1, 'test-proto', null, null),
  { local_id: 1, protocol: 'test-proto', id: null, handshake: null }
);

addFrame('open: with id', 'open',
  encodeOpen(1, 'test-proto', Buffer.from('sub-channel'), null),
  { local_id: 1, protocol: 'test-proto', id: Buffer.from('sub-channel').toString('hex'), handshake: null }
);

addFrame('open: with handshake', 'open',
  encodeOpen(1, 'test-proto', null, Buffer.from('hello')),
  { local_id: 1, protocol: 'test-proto', id: null, handshake: Buffer.from('hello').toString('hex') }
);

addFrame('open: with id and handshake', 'open',
  encodeOpen(1, 'test-proto', Buffer.from([0xde, 0xad]), Buffer.from([0xbe, 0xef])),
  { local_id: 1, protocol: 'test-proto', id: 'dead', handshake: 'beef' }
);

addFrame('open: large local_id (253)', 'open',
  encodeOpen(253, 'big-id', null, null),
  { local_id: 253, protocol: 'big-id', id: null, handshake: null }
);

addFrame('open: large local_id (1000)', 'open',
  encodeOpen(1000, 'big-id', null, null),
  { local_id: 1000, protocol: 'big-id', id: null, handshake: null }
);

addFrame('open: blind-relay protocol', 'open',
  encodeOpen(1, 'blind-relay', null, null),
  { local_id: 1, protocol: 'blind-relay', id: null, handshake: null }
);

addFrame('open: empty protocol', 'open',
  encodeOpen(1, '', null, null),
  { local_id: 1, protocol: '', id: null, handshake: null }
);

// Close frames
addFrame('close: channel 1', 'close',
  encodeClose(1),
  { local_id: 1 }
);

addFrame('close: channel 5', 'close',
  encodeClose(5),
  { local_id: 5 }
);

addFrame('close: large id (300)', 'close',
  encodeClose(300),
  { local_id: 300 }
);

// Reject frames
addFrame('reject: channel 1', 'reject',
  encodeReject(1),
  { remote_id: 1 }
);

addFrame('reject: channel 42', 'reject',
  encodeReject(42),
  { remote_id: 42 }
);

// Message frames (channelId is 1-indexed on wire)
addFrame('message: simple', 'message',
  encodeMessage(1, 0, Buffer.from('hello world')),
  { channel_id: 1, message_type: 0, payload: Buffer.from('hello world').toString('hex') }
);

addFrame('message: type 1', 'message',
  encodeMessage(1, 1, Buffer.from('ping')),
  { channel_id: 1, message_type: 1, payload: Buffer.from('ping').toString('hex') }
);

addFrame('message: empty payload', 'message',
  encodeMessage(1, 0, Buffer.alloc(0)),
  { channel_id: 1, message_type: 0, payload: '' }
);

addFrame('message: channel 3, type 2', 'message',
  encodeMessage(3, 2, Buffer.from([0xff, 0x00, 0xaa, 0x55])),
  { channel_id: 3, message_type: 2, payload: 'ff00aa55' }
);

addFrame('message: large channel id (300)', 'message',
  encodeMessage(300, 0, Buffer.from('data')),
  { channel_id: 300, message_type: 0, payload: Buffer.from('data').toString('hex') }
);

// Batch frames
const item1 = encodeBatchItem(0, Buffer.from('msg-a'));
const item2 = encodeBatchItem(0, Buffer.from('msg-b'));
const item3 = encodeBatchItem(1, Buffer.from('msg-c'));

addFrame('batch: single channel, two messages', 'batch',
  encodeBatch([
    { channelId: 1, data: item1 },
    { channelId: 1, data: item2 },
  ]),
  {
    items: [
      { channel_id: 1, inner_hex: item1.toString('hex') },
      { channel_id: 1, inner_hex: item2.toString('hex') },
    ]
  }
);

addFrame('batch: two channels', 'batch',
  encodeBatch([
    { channelId: 1, data: item1 },
    { channelId: 2, data: item3 },
  ]),
  {
    items: [
      { channel_id: 1, inner_hex: item1.toString('hex') },
      { channel_id: 2, inner_hex: item3.toString('hex') },
    ]
  }
);

addFrame('batch: three messages, channel switch in middle', 'batch',
  encodeBatch([
    { channelId: 1, data: item1 },
    { channelId: 1, data: item2 },
    { channelId: 3, data: item3 },
  ]),
  {
    items: [
      { channel_id: 1, inner_hex: item1.toString('hex') },
      { channel_id: 1, inner_hex: item2.toString('hex') },
      { channel_id: 3, inner_hex: item3.toString('hex') },
    ]
  }
);

// ── Part 2: Live protomux conversation capture ───────────────────────────────
// Create two protomux instances connected by streams, capture all frames.

async function captureConversation() {
  const Protomux = require('protomux');

  const captured = { a_to_b: [], b_to_a: [] };

  // Create paired streams using streamx
  const streamA = new Duplex({
    write(data, cb) {
      captured.a_to_b.push(Buffer.from(data).toString('hex'));
      streamB.push(data);
      cb(null);
    }
  });

  const streamB = new Duplex({
    write(data, cb) {
      captured.b_to_a.push(Buffer.from(data).toString('hex'));
      streamA.push(data);
      cb(null);
    }
  });

  const muxA = new Protomux(streamA);
  const muxB = new Protomux(streamB);

  // Side A opens a channel
  const channelA = muxA.createChannel({
    protocol: 'test-echo',
    onopen() {
      channelA.messages[0].send(Buffer.from('hello from A'));
    },
    onclose() {},
  });
  channelA.addMessage({
    encoding: c.buffer,
    onmessage(msg) {
      // A receives echo
    },
  });
  channelA.open();

  // Side B mirrors
  const channelB = muxB.createChannel({
    protocol: 'test-echo',
    onopen() {},
    onclose() {},
  });
  channelB.addMessage({
    encoding: c.buffer,
    onmessage(msg) {
      // Echo back
      channelB.messages[0].send(Buffer.from('echo: ' + msg.toString()));
    },
  });
  channelB.open();

  // Wait for message exchange
  await new Promise(resolve => setTimeout(resolve, 100));

  // Close channel A
  channelA.close();
  await new Promise(resolve => setTimeout(resolve, 50));

  fixtures.conversations.push({
    label: 'basic open-message-close exchange',
    protocol: 'test-echo',
    a_to_b: captured.a_to_b,
    b_to_a: captured.b_to_a,
  });

  streamA.destroy();
  streamB.destroy();
}

async function main() {
  await captureConversation();

  const output = {
    generated_by: 'protomux golden fixture generator',
    protomux_version: pkgVersion,
    compact_encoding_version: require('./node_modules/compact-encoding/package.json').version,
    fixtures,
  };

  const outDir = path.resolve(__dirname, '..', 'interop');
  fs.mkdirSync(outDir, { recursive: true });
  const outPath = path.join(outDir, 'protomux-fixtures.json');
  fs.writeFileSync(outPath, JSON.stringify(output, null, 2), 'utf8');

  console.log(`Generated ${fixtures.frames.length} frame fixtures + ${fixtures.conversations.length} conversations → ${outPath}`);
}

main().catch(err => { console.error(err); process.exit(1); });
