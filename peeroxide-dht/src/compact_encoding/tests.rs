use super::*;

#[allow(dead_code)]
fn encode_value(f: impl Fn(&mut State)) -> Vec<u8> {
    let mut state = State::new();
    let mut pre = State::new();
    f(&mut pre);
    state.end = pre.end;
    state.alloc();
    f(&mut state);
    state.buffer
}

fn encode_value_with_pre(
    pre_fn: impl FnOnce(&mut State),
    enc_fn: impl FnOnce(&mut State),
) -> Vec<u8> {
    let mut state = State::new();
    pre_fn(&mut state);
    state.alloc();
    enc_fn(&mut state);
    state.buffer
}

#[test]
fn uint8_roundtrip() {
    for val in [0u8, 1, 127, 128, 255] {
        let buf = encode_value_with_pre(
            |s| preencode_uint8(s, val),
            |s| encode_uint8(s, val),
        );
        assert_eq!(buf, vec![val]);
        let mut state = State::from_buffer(&buf);
        assert_eq!(decode_uint8(&mut state).unwrap(), val);
    }
}

#[test]
fn uint16_little_endian() {
    let buf = encode_value_with_pre(
        |s| preencode_uint16(s, 0x0102),
        |s| encode_uint16(s, 0x0102),
    );
    assert_eq!(buf, vec![0x02, 0x01]);
    let mut state = State::from_buffer(&buf);
    assert_eq!(decode_uint16(&mut state).unwrap(), 0x0102);
}

#[test]
fn uint24_little_endian() {
    let buf = encode_value_with_pre(
        |s| preencode_uint24(s, 0x010203),
        |s| encode_uint24(s, 0x010203),
    );
    assert_eq!(buf, vec![0x03, 0x02, 0x01]);
    let mut state = State::from_buffer(&buf);
    assert_eq!(decode_uint24(&mut state).unwrap(), 0x010203);
}

#[test]
fn uint32_little_endian() {
    let buf = encode_value_with_pre(
        |s| preencode_uint32(s, 0x01020304),
        |s| encode_uint32(s, 0x01020304),
    );
    assert_eq!(buf, vec![0x04, 0x03, 0x02, 0x01]);
    let mut state = State::from_buffer(&buf);
    assert_eq!(decode_uint32(&mut state).unwrap(), 0x01020304);
}

#[test]
fn uint64_little_endian() {
    let buf = encode_value_with_pre(
        |s| preencode_uint64(s, 0x0102030405060708),
        |s| encode_uint64(s, 0x0102030405060708),
    );
    assert_eq!(buf, vec![0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01]);
    let mut state = State::from_buffer(&buf);
    assert_eq!(decode_uint64(&mut state).unwrap(), 0x0102030405060708);
}

#[test]
fn uint_varint_1_byte() {
    for val in [0u64, 1, 100, 252] {
        let buf = encode_value_with_pre(
            |s| preencode_uint(s, val),
            |s| encode_uint(s, val),
        );
        assert_eq!(buf.len(), 1);
        assert_eq!(buf[0], val as u8);
        let mut state = State::from_buffer(&buf);
        assert_eq!(decode_uint(&mut state).unwrap(), val);
    }
}

#[test]
fn uint_varint_3_byte() {
    for val in [253u64, 1000, 65535] {
        let buf = encode_value_with_pre(
            |s| preencode_uint(s, val),
            |s| encode_uint(s, val),
        );
        assert_eq!(buf.len(), 3);
        assert_eq!(buf[0], 0xfd);
        let mut state = State::from_buffer(&buf);
        assert_eq!(decode_uint(&mut state).unwrap(), val);
    }
}

#[test]
fn uint_varint_5_byte() {
    for val in [65536u64, 100_000, 0xffffffff] {
        let buf = encode_value_with_pre(
            |s| preencode_uint(s, val),
            |s| encode_uint(s, val),
        );
        assert_eq!(buf.len(), 5);
        assert_eq!(buf[0], 0xfe);
        let mut state = State::from_buffer(&buf);
        assert_eq!(decode_uint(&mut state).unwrap(), val);
    }
}

#[test]
fn uint_varint_9_byte() {
    let val = 0x100000000u64;
    let buf = encode_value_with_pre(
        |s| preencode_uint(s, val),
        |s| encode_uint(s, val),
    );
    assert_eq!(buf.len(), 9);
    assert_eq!(buf[0], 0xff);
    let mut state = State::from_buffer(&buf);
    assert_eq!(decode_uint(&mut state).unwrap(), val);
}

#[test]
fn zigzag_encoding() {
    assert_eq!(zigzag_encode(0), 0);
    assert_eq!(zigzag_encode(-1), 1);
    assert_eq!(zigzag_encode(1), 2);
    assert_eq!(zigzag_encode(-2), 3);
    assert_eq!(zigzag_encode(2), 4);

    assert_eq!(zigzag_decode(0), 0);
    assert_eq!(zigzag_decode(1), -1);
    assert_eq!(zigzag_decode(2), 1);
    assert_eq!(zigzag_decode(3), -2);
    assert_eq!(zigzag_decode(4), 2);
}

#[test]
fn int_roundtrip() {
    for val in [0i64, 1, -1, 127, -128, 1000, -1000, i64::MAX, i64::MIN] {
        let buf = encode_value_with_pre(
            |s| preencode_int(s, val),
            |s| encode_int(s, val),
        );
        let mut state = State::from_buffer(&buf);
        assert_eq!(decode_int(&mut state).unwrap(), val);
    }
}

#[test]
fn float64_roundtrip() {
    for val in [0.0f64, 1.5, -1.5, std::f64::consts::PI, f64::MAX, f64::MIN] {
        let buf = encode_value_with_pre(
            |s| preencode_float64(s, val),
            |s| encode_float64(s, val),
        );
        let mut state = State::from_buffer(&buf);
        assert_eq!(decode_float64(&mut state).unwrap(), val);
    }
}

#[test]
fn bool_roundtrip() {
    for val in [true, false] {
        let buf = encode_value_with_pre(
            |s| preencode_bool(s, val),
            |s| encode_bool(s, val),
        );
        let mut state = State::from_buffer(&buf);
        assert_eq!(decode_bool(&mut state).unwrap(), val);
    }
}

#[test]
fn buffer_some() {
    let data = b"hello world";
    let buf = encode_value_with_pre(
        |s| preencode_buffer(s, Some(data.as_slice())),
        |s| encode_buffer(s, Some(data.as_slice())),
    );
    let mut state = State::from_buffer(&buf);
    assert_eq!(decode_buffer(&mut state).unwrap(), Some(data.to_vec()));
}

#[test]
fn buffer_none() {
    let buf = encode_value_with_pre(
        |s| preencode_buffer(s, None),
        |s| encode_buffer(s, None),
    );
    assert_eq!(buf, vec![0x00]);
    let mut state = State::from_buffer(&buf);
    assert_eq!(decode_buffer(&mut state).unwrap(), None);
}

#[test]
fn string_roundtrip() {
    for val in ["", "hello", "hello world 🌍", "a".repeat(1000).as_str()] {
        let buf = encode_value_with_pre(
            |s| preencode_string(s, val),
            |s| encode_string(s, val),
        );
        let mut state = State::from_buffer(&buf);
        assert_eq!(decode_string(&mut state).unwrap(), val);
    }
}

#[test]
fn fixed32_roundtrip() {
    let data = [42u8; 32];
    let buf = encode_value_with_pre(
        |s| { preencode_fixed32(s, &data).unwrap(); },
        |s| encode_fixed32(s, &data),
    );
    assert_eq!(buf.len(), 32);
    let mut state = State::from_buffer(&buf);
    assert_eq!(decode_fixed32(&mut state).unwrap(), data);
}

#[test]
fn fixed64_roundtrip() {
    let data = [99u8; 64];
    let buf = encode_value_with_pre(
        |s| { preencode_fixed64(s, &data).unwrap(); },
        |s| encode_fixed64(s, &data),
    );
    assert_eq!(buf.len(), 64);
    let mut state = State::from_buffer(&buf);
    assert_eq!(decode_fixed64(&mut state).unwrap(), data);
}

#[test]
fn ipv4_roundtrip() {
    for addr in ["127.0.0.1", "192.168.1.1", "0.0.0.0", "255.255.255.255"] {
        let buf = encode_value_with_pre(
            |s| preencode_ipv4(s, addr),
            |s| encode_ipv4(s, addr).unwrap(),
        );
        assert_eq!(buf.len(), 4);
        let mut state = State::from_buffer(&buf);
        assert_eq!(decode_ipv4(&mut state).unwrap(), addr);
    }
}

#[test]
fn ipv6_roundtrip() {
    let buf = encode_value_with_pre(
        |s| preencode_ipv6(s, "::1"),
        |s| encode_ipv6(s, "::1").unwrap(),
    );
    assert_eq!(buf.len(), 16);
    let mut state = State::from_buffer(&buf);
    let decoded = decode_ipv6(&mut state).unwrap();
    assert_eq!(decoded, "::1");
}

#[test]
fn ip_dual_v4() {
    let addr = "192.168.1.1";
    let buf = encode_value_with_pre(
        |s| preencode_ip(s, addr),
        |s| encode_ip(s, addr).unwrap(),
    );
    assert_eq!(buf[0], 4);
    assert_eq!(buf.len(), 5);
    let mut state = State::from_buffer(&buf);
    assert_eq!(decode_ip(&mut state).unwrap(), addr);
}

#[test]
fn ip_dual_v6() {
    let addr = "::1";
    let buf = encode_value_with_pre(
        |s| preencode_ip(s, addr),
        |s| encode_ip(s, addr).unwrap(),
    );
    assert_eq!(buf[0], 6);
    assert_eq!(buf.len(), 17);
    let mut state = State::from_buffer(&buf);
    assert_eq!(decode_ip(&mut state).unwrap(), "::1");
}

#[test]
fn uint_array_roundtrip() {
    let arr = vec![0u64, 1, 252, 253, 65535, 65536, 0xffffffff, 0x100000000];
    let buf = encode_value_with_pre(
        |s| preencode_uint_array(s, &arr),
        |s| encode_uint_array(s, &arr),
    );
    let mut state = State::from_buffer(&buf);
    assert_eq!(decode_uint_array(&mut state).unwrap(), arr);
}

#[test]
fn string_array_roundtrip() {
    let arr = vec!["hello", "world", "", "🌍"];
    let buf = encode_value_with_pre(
        |s| preencode_string_array(s, &arr),
        |s| encode_string_array(s, &arr),
    );
    let mut state = State::from_buffer(&buf);
    assert_eq!(decode_string_array(&mut state).unwrap(), arr.iter().map(|s| s.to_string()).collect::<Vec<_>>());
}

#[test]
fn ipv4_address_roundtrip() {
    let buf = encode_value_with_pre(
        |s| preencode_ipv4_address(s, "10.0.0.1", 8080),
        |s| encode_ipv4_address(s, "10.0.0.1", 8080).unwrap(),
    );
    assert_eq!(buf.len(), 6);
    let mut state = State::from_buffer(&buf);
    let (addr, port) = decode_ipv4_address(&mut state).unwrap();
    assert_eq!(addr, "10.0.0.1");
    assert_eq!(port, 8080);
}

#[test]
fn out_of_bounds_error() {
    let mut state = State::from_buffer(&[]);
    assert!(decode_uint8(&mut state).is_err());

    let mut state = State::from_buffer(&[0x01]);
    assert!(decode_uint16(&mut state).is_err());
}

#[test]
fn multiple_values_sequential() {
    let mut state = State::new();
    preencode_uint(state.borrow_mut(), 42);
    preencode_string(&mut state, "hello");
    preencode_bool(&mut state, true);
    state.alloc();
    encode_uint(&mut state, 42);
    encode_string(&mut state, "hello");
    encode_bool(&mut state, true);

    let mut dec = State::from_buffer(&state.buffer);
    assert_eq!(decode_uint(&mut dec).unwrap(), 42);
    assert_eq!(decode_string(&mut dec).unwrap(), "hello");
    assert!(decode_bool(&mut dec).unwrap());
    assert_eq!(dec.start, dec.end);
}

trait BorrowMut {
    fn borrow_mut(&mut self) -> &mut Self {
        self
    }
}

impl BorrowMut for State {}
