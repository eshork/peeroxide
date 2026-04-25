'use strict';

const c = require('compact-encoding');
const fs = require('fs');
const path = require('path');

const pkgVersion = require('./node_modules/compact-encoding/package.json').version;

const fixtures = [];

function add(type, label, value, encodedBuf) {
  let jsonValue;
  if (typeof value === 'bigint') {
    jsonValue = value.toString();
  } else if (Buffer.isBuffer(value)) {
    jsonValue = value.toString('hex');
  } else if (value === null) {
    jsonValue = null;
  } else if (Array.isArray(value)) {
    jsonValue = value.map(v => {
      if (typeof v === 'bigint') return v.toString();
      if (Buffer.isBuffer(v)) return v.toString('hex');
      return v;
    });
  } else {
    jsonValue = value;
  }

  fixtures.push({
    type,
    label,
    value: jsonValue,
    hex: encodedBuf.toString('hex'),
  });
}

// ── uint8 ────────────────────────────────────────────────────────────────────
add('uint8', 'uint8 zero',    0,   c.encode(c.uint8, 0));
add('uint8', 'uint8 one',     1,   c.encode(c.uint8, 1));
add('uint8', 'uint8 127',   127,   c.encode(c.uint8, 127));
add('uint8', 'uint8 128',   128,   c.encode(c.uint8, 128));
add('uint8', 'uint8 max',   255,   c.encode(c.uint8, 255));

// ── uint16 ───────────────────────────────────────────────────────────────────
add('uint16', 'uint16 zero',       0,        c.encode(c.uint16, 0));
add('uint16', 'uint16 one',        1,        c.encode(c.uint16, 1));
add('uint16', 'uint16 0x0102', 0x0102,       c.encode(c.uint16, 0x0102));
add('uint16', 'uint16 max',    0xffff,       c.encode(c.uint16, 0xffff));

// ── uint24 ───────────────────────────────────────────────────────────────────
add('uint24', 'uint24 zero',           0,          c.encode(c.uint24, 0));
add('uint24', 'uint24 0x010203',   0x010203,       c.encode(c.uint24, 0x010203));
add('uint24', 'uint24 max',        0xffffff,       c.encode(c.uint24, 0xffffff));

// ── uint32 ───────────────────────────────────────────────────────────────────
add('uint32', 'uint32 zero',             0,           c.encode(c.uint32, 0));
add('uint32', 'uint32 one',              1,           c.encode(c.uint32, 1));
add('uint32', 'uint32 0x01020304', 0x01020304,        c.encode(c.uint32, 0x01020304));
add('uint32', 'uint32 max',        0xffffffff,        c.encode(c.uint32, 0xffffffff));

// ── uint64 (8-byte LE; use biguint64 codec for BigInt precision) ──────────────
add('uint64', 'uint64 zero',                0n,                   c.encode(c.biguint64, 0n));
add('uint64', 'uint64 one',                 1n,                   c.encode(c.biguint64, 1n));
add('uint64', 'uint64 0x0102030405060708',  0x0102030405060708n,  c.encode(c.biguint64, 0x0102030405060708n));

// ── uint (varint) ─────────────────────────────────────────────────────────────
add('uint', 'uint varint zero',       0,           c.encode(c.uint, 0));
add('uint', 'uint varint one',        1,           c.encode(c.uint, 1));
add('uint', 'uint varint 100',      100,           c.encode(c.uint, 100));
add('uint', 'uint varint 252',      252,           c.encode(c.uint, 252));
add('uint', 'uint varint 253',      253,           c.encode(c.uint, 253));
add('uint', 'uint varint 1000',    1000,           c.encode(c.uint, 1000));
add('uint', 'uint varint 65535',  65535,           c.encode(c.uint, 65535));
add('uint', 'uint varint 65536',  65536,           c.encode(c.uint, 65536));
add('uint', 'uint varint 100000', 100000,          c.encode(c.uint, 100000));
add('uint', 'uint varint 0xffffffff', 0xffffffff,  c.encode(c.uint, 0xffffffff));
add('uint', 'uint varint 0x100000000', 0x100000000, c.encode(c.uint, 0x100000000));

// ── int (zigzag varint) ───────────────────────────────────────────────────────
add('int', 'int zigzag zero',    0,     c.encode(c.int, 0));
add('int', 'int zigzag one',     1,     c.encode(c.int, 1));
add('int', 'int zigzag -1',     -1,     c.encode(c.int, -1));
add('int', 'int zigzag two',     2,     c.encode(c.int, 2));
add('int', 'int zigzag -2',     -2,     c.encode(c.int, -2));
add('int', 'int zigzag 127',   127,     c.encode(c.int, 127));
add('int', 'int zigzag -128', -128,     c.encode(c.int, -128));
add('int', 'int zigzag 1000', 1000,     c.encode(c.int, 1000));
add('int', 'int zigzag -1000', -1000,   c.encode(c.int, -1000));

// ── int8 ──────────────────────────────────────────────────────────────────────
add('int8', 'int8 zero',    0,     c.encode(c.int8, 0));
add('int8', 'int8 one',     1,     c.encode(c.int8, 1));
add('int8', 'int8 -1',     -1,     c.encode(c.int8, -1));
add('int8', 'int8 max',   127,     c.encode(c.int8, 127));
add('int8', 'int8 min',  -128,     c.encode(c.int8, -128));

// ── int16 ─────────────────────────────────────────────────────────────────────
add('int16', 'int16 zero',     0,        c.encode(c.int16, 0));
add('int16', 'int16 one',      1,        c.encode(c.int16, 1));
add('int16', 'int16 -1',      -1,        c.encode(c.int16, -1));
add('int16', 'int16 max',  0x7fff,       c.encode(c.int16, 0x7fff));
add('int16', 'int16 min', -0x8000,       c.encode(c.int16, -0x8000));

// ── int32 ─────────────────────────────────────────────────────────────────────
add('int32', 'int32 zero',          0,          c.encode(c.int32, 0));
add('int32', 'int32 one',           1,          c.encode(c.int32, 1));
add('int32', 'int32 -1',           -1,          c.encode(c.int32, -1));
add('int32', 'int32 max',   0x7fffffff,         c.encode(c.int32, 0x7fffffff));
add('int32', 'int32 min',  -0x80000000,         c.encode(c.int32, -0x80000000));

// ── int64 (zigzag 8-byte LE; use bigint64 codec for BigInt precision) ─────────
add('int64', 'int64 zero',  0n,   c.encode(c.bigint64, 0n));
add('int64', 'int64 one',   1n,   c.encode(c.bigint64, 1n));
add('int64', 'int64 -1',   -1n,   c.encode(c.bigint64, -1n));
add('int64', 'int64 large positive', 0x7fffffffffffffffn,  c.encode(c.bigint64, 0x7fffffffffffffffn));
add('int64', 'int64 large negative', -0x8000000000000000n, c.encode(c.bigint64, -0x8000000000000000n));

// ── float32 ───────────────────────────────────────────────────────────────────
add('float32', 'float32 zero',    0.0,   c.encode(c.float32, 0.0));
add('float32', 'float32 1.5',     1.5,   c.encode(c.float32, 1.5));
add('float32', 'float32 -1.5',   -1.5,   c.encode(c.float32, -1.5));
add('float32', 'float32 3.14',   3.14,   c.encode(c.float32, 3.14));

// ── float64 ───────────────────────────────────────────────────────────────────
add('float64', 'float64 zero',           0.0,              c.encode(c.float64, 0.0));
add('float64', 'float64 1.5',            1.5,              c.encode(c.float64, 1.5));
add('float64', 'float64 -1.5',          -1.5,              c.encode(c.float64, -1.5));
add('float64', 'float64 pi',        Math.PI,               c.encode(c.float64, Math.PI));
add('float64', 'float64 MAX_VALUE', Number.MAX_VALUE,      c.encode(c.float64, Number.MAX_VALUE));
add('float64', 'float64 MIN_VALUE', Number.MIN_VALUE,      c.encode(c.float64, Number.MIN_VALUE));

// ── bool ──────────────────────────────────────────────────────────────────────
add('bool', 'bool true',   true,   c.encode(c.bool, true));
add('bool', 'bool false',  false,  c.encode(c.bool, false));

// ── buffer ────────────────────────────────────────────────────────────────────
add('buffer', 'buffer null',              null,                        c.encode(c.buffer, null));
add('buffer', 'buffer empty',             Buffer.alloc(0),             c.encode(c.buffer, Buffer.alloc(0)));
add('buffer', 'buffer hello world',       Buffer.from('hello world'),  c.encode(c.buffer, Buffer.from('hello world')));
add('buffer', 'buffer binary 0xff00ff',   Buffer.from([0xff, 0x00, 0xff]), c.encode(c.buffer, Buffer.from([0xff, 0x00, 0xff])));

// ── string ────────────────────────────────────────────────────────────────────
add('string', 'string empty',       '',                      c.encode(c.string, ''));
add('string', 'string hello',       'hello',                 c.encode(c.string, 'hello'));
add('string', 'string hello world emoji', 'hello world 🌍', c.encode(c.string, 'hello world 🌍'));
add('string', 'string 1000 a chars', 'a'.repeat(1000),      c.encode(c.string, 'a'.repeat(1000)));

// ── fixed32 ───────────────────────────────────────────────────────────────────
add('fixed32', 'fixed32 all-42',    Buffer.alloc(32, 42),   c.encode(c.fixed32, Buffer.alloc(32, 42)));
add('fixed32', 'fixed32 all-zero',  Buffer.alloc(32, 0),    c.encode(c.fixed32, Buffer.alloc(32, 0)));
add('fixed32', 'fixed32 all-0xff',  Buffer.alloc(32, 0xff), c.encode(c.fixed32, Buffer.alloc(32, 0xff)));

// ── fixed64 ───────────────────────────────────────────────────────────────────
add('fixed64', 'fixed64 all-99',    Buffer.alloc(64, 99),   c.encode(c.fixed64, Buffer.alloc(64, 99)));
add('fixed64', 'fixed64 all-zero',  Buffer.alloc(64, 0),    c.encode(c.fixed64, Buffer.alloc(64, 0)));

// ── ipv4 ──────────────────────────────────────────────────────────────────────
add('ipv4', 'ipv4 loopback',        '127.0.0.1',       c.encode(c.ipv4, '127.0.0.1'));
add('ipv4', 'ipv4 private',         '192.168.1.1',     c.encode(c.ipv4, '192.168.1.1'));
add('ipv4', 'ipv4 zero',            '0.0.0.0',         c.encode(c.ipv4, '0.0.0.0'));
add('ipv4', 'ipv4 broadcast',       '255.255.255.255', c.encode(c.ipv4, '255.255.255.255'));

// ── ipv6 ──────────────────────────────────────────────────────────────────────
add('ipv6', 'ipv6 loopback',        '::1',                   c.encode(c.ipv6, '::1'));
add('ipv6', 'ipv6 mapped ipv4',     '::ffff:c0a8:0101',    c.encode(c.ipv6, '::ffff:c0a8:0101'));
add('ipv6', 'ipv6 link-local',      'fe80::1',               c.encode(c.ipv6, 'fe80::1'));
add('ipv6', 'ipv6 documentation',   '2001:db8::1',           c.encode(c.ipv6, '2001:db8::1'));

// ── ip (dual-stack: ipv4 or ipv6) ────────────────────────────────────────────
add('ip', 'ip ipv4 private',        '192.168.1.1',   c.encode(c.ip, '192.168.1.1'));
add('ip', 'ip ipv6 loopback',       '::1',           c.encode(c.ip, '::1'));
add('ip', 'ip ipv4 10.0.0.1',       '10.0.0.1',      c.encode(c.ip, '10.0.0.1'));
add('ip', 'ip ipv6 documentation',  '2001:db8::1',   c.encode(c.ip, '2001:db8::1'));

// ── ipv4Address ───────────────────────────────────────────────────────────────
add('ipv4Address', 'ipv4Address 10.0.0.1:8080',          { host: '10.0.0.1',       port: 8080  }, c.encode(c.ipv4Address, { host: '10.0.0.1',       port: 8080  }));
add('ipv4Address', 'ipv4Address 0.0.0.0:0',              { host: '0.0.0.0',        port: 0     }, c.encode(c.ipv4Address, { host: '0.0.0.0',        port: 0     }));
add('ipv4Address', 'ipv4Address 255.255.255.255:65535',   { host: '255.255.255.255', port: 65535}, c.encode(c.ipv4Address, { host: '255.255.255.255', port: 65535 }));

// ── uint array ────────────────────────────────────────────────────────────────
add('uint_array', 'uint array empty',       [],                                     c.encode(c.array(c.uint), []));
add('uint_array', 'uint array single zero', [0],                                    c.encode(c.array(c.uint), [0]));
add('uint_array', 'uint array diverse',     [0, 1, 252, 253, 65535, 65536, 0xffffffff], c.encode(c.array(c.uint), [0, 1, 252, 253, 65535, 65536, 0xffffffff]));

// ── string array ──────────────────────────────────────────────────────────────
add('string_array', 'string array empty',        [],                         c.encode(c.array(c.string), []));
add('string_array', 'string array single',       ['hello'],                  c.encode(c.array(c.string), ['hello']));
add('string_array', 'string array diverse',      ['hello', 'world', '', '🌍'], c.encode(c.array(c.string), ['hello', 'world', '', '🌍']));

// ── Output ────────────────────────────────────────────────────────────────────
const output = {
  generated_by: 'compact-encoding npm package',
  version: pkgVersion,
  fixtures,
};

const outDir = path.resolve(__dirname, '..', 'interop');
fs.mkdirSync(outDir, { recursive: true });
const outPath = path.join(outDir, 'golden-fixtures.json');
fs.writeFileSync(outPath, JSON.stringify(output, null, 2), 'utf8');

console.log(`Generated ${fixtures.length} fixtures → ${outPath}`);
