pub const HEADER_SIZE: usize = 20;
pub const MAGIC: u8 = 0xFF;
pub const VERSION: u8 = 1;

pub const FLAG_DATA: u8 = 0x01;
pub const FLAG_END: u8 = 0x02;
pub const FLAG_SACK: u8 = 0x04;
pub const FLAG_MESSAGE: u8 = 0x08;
pub const FLAG_DESTROY: u8 = 0x10;
pub const FLAG_HEARTBEAT: u8 = 0x20;

#[derive(Debug, thiserror::Error)]
pub enum HeaderError {
    #[error("packet too short: {0} bytes (minimum {HEADER_SIZE})")]
    TooShort(usize),
    #[error("bad magic byte: 0x{0:02X} (expected 0xFF)")]
    BadMagic(u8),
    #[error("unsupported version: {0} (expected {VERSION})")]
    BadVersion(u8),
    #[error("invalid SACK data: length {0} is not a multiple of 8")]
    InvalidSack(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub type_flags: u8,
    pub data_offset: u8,
    pub remote_id: u32,
    pub recv_window: u32,
    pub seq: u32,
    pub ack: u32,
}

impl Header {
    pub fn encode(&self) -> [u8; HEADER_SIZE] {
        let mut buf = [0u8; HEADER_SIZE];
        self.encode_into(&mut buf);
        buf
    }

    pub fn encode_into(&self, buf: &mut [u8]) {
        buf[0] = MAGIC;
        buf[1] = VERSION;
        buf[2] = self.type_flags;
        buf[3] = self.data_offset;
        buf[4..8].copy_from_slice(&self.remote_id.to_le_bytes());
        buf[8..12].copy_from_slice(&self.recv_window.to_le_bytes());
        buf[12..16].copy_from_slice(&self.seq.to_le_bytes());
        buf[16..20].copy_from_slice(&self.ack.to_le_bytes());
    }

    pub fn decode(buf: &[u8]) -> Result<Self, HeaderError> {
        if buf.len() < HEADER_SIZE {
            return Err(HeaderError::TooShort(buf.len()));
        }
        if buf[0] != MAGIC {
            return Err(HeaderError::BadMagic(buf[0]));
        }
        if buf[1] != VERSION {
            return Err(HeaderError::BadVersion(buf[1]));
        }
        Ok(Self {
            type_flags: buf[2],
            data_offset: buf[3],
            remote_id: u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]),
            recv_window: u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]),
            seq: u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]),
            ack: u32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]),
        })
    }

    pub fn payload_offset(&self) -> usize {
        HEADER_SIZE + self.data_offset as usize
    }

    pub fn has_flag(&self, flag: u8) -> bool {
        self.type_flags & flag != 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SackRange {
    pub start: u32,
    pub end: u32,
}

pub fn encode_sack(ranges: &[SackRange], buf: &mut [u8]) -> usize {
    let needed = ranges.len() * 8;
    for (i, range) in ranges.iter().enumerate() {
        let off = i * 8;
        buf[off..off + 4].copy_from_slice(&range.start.to_le_bytes());
        buf[off + 4..off + 8].copy_from_slice(&range.end.to_le_bytes());
    }
    needed
}

pub fn decode_sack(buf: &[u8]) -> Result<Vec<SackRange>, HeaderError> {
    if buf.len() % 8 != 0 {
        return Err(HeaderError::InvalidSack(buf.len()));
    }
    let mut ranges = Vec::with_capacity(buf.len() / 8);
    for chunk in buf.chunks_exact(8) {
        ranges.push(SackRange {
            start: u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]),
            end: u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]),
        });
    }
    Ok(ranges)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_roundtrip() {
        let header = Header {
            type_flags: FLAG_DATA,
            data_offset: 0,
            remote_id: 42,
            recv_window: 4_194_304,
            seq: 0,
            ack: 0,
        };
        let buf = header.encode();
        let decoded = Header::decode(&buf).unwrap();
        assert_eq!(header, decoded);
    }

    #[test]
    fn encode_known_bytes() {
        let header = Header {
            type_flags: FLAG_DATA,
            data_offset: 0,
            remote_id: 42,
            recv_window: 4_194_304,
            seq: 0,
            ack: 0,
        };
        let buf = header.encode();

        assert_eq!(buf[0], 0xFF);
        assert_eq!(buf[1], 1);
        assert_eq!(buf[2], FLAG_DATA);
        assert_eq!(buf[3], 0);
        assert_eq!(&buf[4..8], &[0x2A, 0x00, 0x00, 0x00]);
        assert_eq!(&buf[8..12], &[0x00, 0x00, 0x40, 0x00]);
        assert_eq!(&buf[12..16], &[0x00, 0x00, 0x00, 0x00]);
        assert_eq!(&buf[16..20], &[0x00, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn encode_end_packet() {
        let header = Header {
            type_flags: FLAG_END,
            data_offset: 0,
            remote_id: 42,
            recv_window: 4_194_304,
            seq: 1,
            ack: 0,
        };
        let buf = header.encode();
        assert_eq!(buf[2], FLAG_END);
        assert_eq!(u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]), 1);
    }

    #[test]
    fn decode_too_short() {
        let buf = [0u8; 19];
        let err = Header::decode(&buf).unwrap_err();
        assert!(matches!(err, HeaderError::TooShort(19)));
    }

    #[test]
    fn decode_bad_magic() {
        let mut buf = [0u8; 20];
        buf[0] = 0xFE;
        buf[1] = VERSION;
        let err = Header::decode(&buf).unwrap_err();
        assert!(matches!(err, HeaderError::BadMagic(0xFE)));
    }

    #[test]
    fn decode_bad_version() {
        let mut buf = [0u8; 20];
        buf[0] = MAGIC;
        buf[1] = 99;
        let err = Header::decode(&buf).unwrap_err();
        assert!(matches!(err, HeaderError::BadVersion(99)));
    }

    #[test]
    fn flag_combinations() {
        let header = Header {
            type_flags: FLAG_DATA | FLAG_SACK,
            data_offset: 16,
            remote_id: 1,
            recv_window: 1024,
            seq: 10,
            ack: 5,
        };
        assert!(header.has_flag(FLAG_DATA));
        assert!(header.has_flag(FLAG_SACK));
        assert!(!header.has_flag(FLAG_END));
        assert!(!header.has_flag(FLAG_DESTROY));
    }

    #[test]
    fn payload_offset_no_sack() {
        let header = Header {
            type_flags: FLAG_DATA,
            data_offset: 0,
            remote_id: 0,
            recv_window: 0,
            seq: 0,
            ack: 0,
        };
        assert_eq!(header.payload_offset(), 20);
    }

    #[test]
    fn payload_offset_with_sack() {
        let header = Header {
            type_flags: FLAG_DATA | FLAG_SACK,
            data_offset: 16,
            remote_id: 0,
            recv_window: 0,
            seq: 0,
            ack: 0,
        };
        assert_eq!(header.payload_offset(), 36);
    }

    #[test]
    fn sack_roundtrip() {
        let ranges = vec![
            SackRange { start: 5, end: 10 },
            SackRange { start: 15, end: 20 },
        ];
        let mut buf = [0u8; 16];
        let written = encode_sack(&ranges, &mut buf);
        assert_eq!(written, 16);

        let decoded = decode_sack(&buf).unwrap();
        assert_eq!(ranges, decoded);
    }

    #[test]
    fn sack_empty() {
        let ranges: Vec<SackRange> = vec![];
        let mut buf = [0u8; 0];
        let written = encode_sack(&ranges, &mut buf);
        assert_eq!(written, 0);

        let decoded = decode_sack(&[]).unwrap();
        assert!(decoded.is_empty());
    }

    #[test]
    fn sack_invalid_length() {
        let buf = [0u8; 7];
        let err = decode_sack(&buf).unwrap_err();
        assert!(matches!(err, HeaderError::InvalidSack(7)));
    }

    #[test]
    fn sack_known_bytes() {
        let ranges = vec![SackRange { start: 100, end: 200 }];
        let mut buf = [0u8; 8];
        encode_sack(&ranges, &mut buf);
        assert_eq!(&buf[0..4], &100u32.to_le_bytes());
        assert_eq!(&buf[4..8], &200u32.to_le_bytes());
    }

    #[test]
    fn decode_ignores_trailing_bytes() {
        let header = Header {
            type_flags: FLAG_DATA,
            data_offset: 0,
            remote_id: 7,
            recv_window: 256,
            seq: 99,
            ack: 50,
        };
        let mut buf = [0u8; 30];
        header.encode_into(&mut buf);
        buf[20..].fill(0xAB);

        let decoded = Header::decode(&buf).unwrap();
        assert_eq!(header, decoded);
    }

    #[test]
    fn all_fields_max_values() {
        let header = Header {
            type_flags: 0xFF,
            data_offset: 0xFF,
            remote_id: u32::MAX,
            recv_window: u32::MAX,
            seq: u32::MAX,
            ack: u32::MAX,
        };
        let buf = header.encode();
        let decoded = Header::decode(&buf).unwrap();
        assert_eq!(header, decoded);
    }

    #[test]
    fn encode_into_writes_correct_slice() {
        let header = Header {
            type_flags: FLAG_HEARTBEAT,
            data_offset: 0,
            remote_id: 1000,
            recv_window: 65536,
            seq: 42,
            ack: 41,
        };
        let mut packet = [0u8; 64];
        header.encode_into(&mut packet);

        assert_eq!(packet[0], MAGIC);
        assert_eq!(packet[2], FLAG_HEARTBEAT);
        assert_eq!(
            u32::from_le_bytes([packet[4], packet[5], packet[6], packet[7]]),
            1000
        );
        assert_eq!(packet[20], 0);
    }
}
