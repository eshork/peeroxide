'use strict';

const c = require('compact-encoding');
const peer = require('dht-rpc/lib/peer');
const fs = require('fs');
const path = require('path');

const dhtPkg = require('dht-rpc/package.json');

const REQUEST_ID  = 0x03;
const RESPONSE_ID = 0x13;

function encodeRequest(fields) {
  const { tid, to_host, to_port, id, token, internal, command, target, value } = fields;

  const state = { start: 0, end: 1 + 1 + 2 + 6, buffer: null };

  if (id)     state.end += 32;
  if (token)  state.end += 32;
  c.uint.preencode(state, command);
  if (target) state.end += 32;
  if (value)  c.buffer.preencode(state, value);

  state.buffer = Buffer.allocUnsafe(state.end);
  state.buffer[state.start++] = REQUEST_ID;
  state.buffer[state.start++] =
    (id       ? 1  : 0) |
    (token    ? 2  : 0) |
    (internal ? 4  : 0) |
    (target   ? 8  : 0) |
    (value    ? 16 : 0);

  c.uint16.encode(state, tid);
  peer.ipv4.encode(state, { host: to_host, port: to_port });

  if (id)     c.fixed32.encode(state, id);
  if (token)  c.fixed32.encode(state, token);
  c.uint.encode(state, command);
  if (target) c.fixed32.encode(state, target);
  if (value)  c.buffer.encode(state, value);

  return state.buffer;
}

function encodeResponse(fields) {
  const { tid, to_host, to_port, id, token, closer_nodes, error, value } = fields;
  const hasCloser = closer_nodes && closer_nodes.length > 0;
  const hasError  = error > 0;

  const peerCloserNodes = (closer_nodes || []).map(n => ({ host: n.host, port: n.port }));

  const state = { start: 0, end: 1 + 1 + 2 + 6, buffer: null };

  if (id)         state.end += 32;
  if (token)      state.end += 32;
  if (hasCloser)  peer.ipv4Array.preencode(state, peerCloserNodes);
  if (hasError)   c.uint.preencode(state, error);
  if (value)      c.buffer.preencode(state, value);

  state.buffer = Buffer.allocUnsafe(state.end);
  state.buffer[state.start++] = RESPONSE_ID;
  state.buffer[state.start++] =
    (id         ? 1  : 0) |
    (token      ? 2  : 0) |
    (hasCloser  ? 4  : 0) |
    (hasError   ? 8  : 0) |
    (value      ? 16 : 0);

  c.uint16.encode(state, tid);
  peer.ipv4.encode(state, { host: to_host, port: to_port });

  if (id)        c.fixed32.encode(state, id);
  if (token)     c.fixed32.encode(state, token);
  if (hasCloser) peer.ipv4Array.encode(state, peerCloserNodes);
  if (hasError)  c.uint.encode(state, error);
  if (value)     c.buffer.encode(state, value);

  return state.buffer;
}

const fixtures = [];

function addRequest(label, fields) {
  const buf = encodeRequest(fields);
  fixtures.push({
    type: 'request',
    label,
    fields: {
      tid:      fields.tid,
      to_host:  fields.to_host,
      to_port:  fields.to_port,
      id:       fields.id    ? fields.id.toString('hex')    : null,
      token:    fields.token ? fields.token.toString('hex') : null,
      internal: fields.internal,
      command:  fields.command,
      target:   fields.target ? fields.target.toString('hex') : null,
      value:    fields.value  ? fields.value.toString('hex')  : null,
    },
    hex: buf.toString('hex'),
  });
}

function addResponse(label, fields) {
  const buf = encodeResponse(fields);
  fixtures.push({
    type: 'response',
    label,
    fields: {
      tid:          fields.tid,
      to_host:      fields.to_host,
      to_port:      fields.to_port,
      id:           fields.id    ? fields.id.toString('hex')    : null,
      token:        fields.token ? fields.token.toString('hex') : null,
      closer_nodes: (fields.closer_nodes || []).map(n => ({ host: n.host, port: n.port })),
      error:        fields.error || 0,
      value:        fields.value ? fields.value.toString('hex') : null,
    },
    hex: buf.toString('hex'),
  });
}

addRequest('req_minimal', {
  tid:      1,
  to_host:  '127.0.0.1',
  to_port:  8080,
  id:       null,
  token:    null,
  internal: false,
  command:  0,
  target:   null,
  value:    null,
});

addRequest('req_internal_ping', {
  tid:      42,
  to_host:  '10.0.0.1',
  to_port:  49737,
  id:       null,
  token:    null,
  internal: true,
  command:  0,
  target:   null,
  value:    null,
});

addRequest('req_find_node', {
  tid:      100,
  to_host:  '192.168.1.1',
  to_port:  3000,
  id:       null,
  token:    null,
  internal: true,
  command:  2,
  target:   Buffer.alloc(32, 0xAA),
  value:    null,
});

addRequest('req_full', {
  tid:      1000,
  to_host:  '10.0.0.1',
  to_port:  8080,
  id:       Buffer.alloc(32, 0x11),
  token:    Buffer.alloc(32, 0x22),
  internal: true,
  command:  2,
  target:   Buffer.alloc(32, 0x33),
  value:    Buffer.from([1, 2, 3, 4]),
});

addRequest('req_external_with_value', {
  tid:      500,
  to_host:  '172.16.0.1',
  to_port:  9999,
  id:       null,
  token:    null,
  internal: false,
  command:  5,
  target:   Buffer.alloc(32, 0x44),
  value:    Buffer.from('hello'),
});

addResponse('res_minimal', {
  tid:          1,
  to_host:      '127.0.0.1',
  to_port:      8080,
  id:           null,
  token:        null,
  closer_nodes: [],
  error:        0,
  value:        null,
});

addResponse('res_with_error', {
  tid:          100,
  to_host:      '10.0.0.1',
  to_port:      5000,
  id:           null,
  token:        null,
  closer_nodes: [],
  error:        42,
  value:        null,
});

addResponse('res_with_closer_nodes', {
  tid:      200,
  to_host:  '192.168.1.1',
  to_port:  3000,
  id:       Buffer.alloc(32, 0x55),
  token:    Buffer.alloc(32, 0x66),
  closer_nodes: [
    { host: '10.0.0.1', port: 8080 },
    { host: '10.0.0.2', port: 9090 },
  ],
  error: 0,
  value: Buffer.from('hello'),
});

addResponse('res_full', {
  tid:      65535,
  to_host:  '10.0.0.1',
  to_port:  8080,
  id:       Buffer.alloc(32, 0x77),
  token:    Buffer.alloc(32, 0x88),
  closer_nodes: [
    { host: '192.168.1.1', port: 3000 },
  ],
  error: 0,
  value: Buffer.from([0xDE, 0xAD, 0xBE, 0xEF]),
});

const peerIdBuf = peer.id('127.0.0.1', 8080);
fixtures.push({
  type:  'peer_id',
  label: 'peer_id_127.0.0.1:8080',
  fields: {
    host: '127.0.0.1',
    port: 8080,
  },
  hex: peerIdBuf.toString('hex'),
});

const outDir  = path.resolve(__dirname, '../interop');
const outFile = path.join(outDir, 'dht-rpc-fixtures.json');

fs.mkdirSync(outDir, { recursive: true });

const output = {
  generator:       'generate-dht-golden.js',
  dht_rpc_version: dhtPkg.version,
  fixtures,
};

fs.writeFileSync(outFile, JSON.stringify(output, null, 2) + '\n');
console.log(`Written ${fixtures.length} fixtures to ${outFile}`);
