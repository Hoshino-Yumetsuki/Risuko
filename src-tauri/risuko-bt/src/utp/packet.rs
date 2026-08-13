//! µTP (BEP-29) packet header + extension codec Wire layout of the fixed 20-byte header (all multi-byte fields big-endian): ```text 0       4       8               16              24              32 +-------+-------+---------------+-------------------------------+ | type  | ver=1 | extension     | connection_id                 | +-------+-------+---------------+-------------------------------+ | timestamp_microseconds                                        | +---------------------------------------------------------------+ | timestamp_difference_microseconds                             | +---------------------------------------------------------------+ | wnd_size                                                      | +-------------------------------+-------------------------------+ | seq_nr                        | ack_nr                        | +-------------------------------+-------------------------------+ ``` `type` is the high nibble of byte 0, `ver` the low nibble; a non-zero `extension` byte introduces a linked list of `(next_ext, len, data)` records following the header, of which we only care about Selective ACK (extension type 1) whose `data` is a little-endian-bit bitmask of sequence numbers received past `ack_nr`

use std::io;

/// µTP protocol version we speak (the only version defined by BEP-29)
pub const UTP_VERSION: u8 = 1;
/// Length of the fixed µTP header; extensions and payload follow
pub const HEADER_LEN: usize = 20;
/// Extension id for Selective ACK (BEP-29)
const EXT_SELECTIVE_ACK: u8 = 1;

/// µTP packet type (the high nibble of the first header byte)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketType {
    Data = 0,
    Fin = 1,
    State = 2,
    Reset = 3,
    Syn = 4,
}

impl PacketType {
    fn from_nibble(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Data),
            1 => Some(Self::Fin),
            2 => Some(Self::State),
            3 => Some(Self::Reset),
            4 => Some(Self::Syn),
            _ => None,
        }
    }
}

/// A decoded µTP header; payload bytes are returned separately by [`UtpHeader::decode`] so the header type stays `Copy`-cheap to clone
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UtpHeader {
    pub packet_type: PacketType,
    pub connection_id: u16,
    pub timestamp_micros: u32,
    pub timestamp_diff_micros: u32,
    pub wnd_size: u32,
    pub seq_nr: u16,
    pub ack_nr: u16,
    pub selective_ack: Option<Vec<u8>>,
}

impl UtpHeader {
    /// Serialize this header plus `payload` into a single datagram body
    pub fn encode(&self, payload: &[u8]) -> Vec<u8> {
        let sack = self.selective_ack.as_deref().filter(|s| !s.is_empty());
        let ext_len = sack.map_or(0, |s| 2 + s.len());
        let mut out = Vec::with_capacity(HEADER_LEN + ext_len + payload.len());
        out.push(((self.packet_type as u8) << 4) | UTP_VERSION);
        out.push(if sack.is_some() { EXT_SELECTIVE_ACK } else { 0 });
        out.extend_from_slice(&self.connection_id.to_be_bytes());
        out.extend_from_slice(&self.timestamp_micros.to_be_bytes());
        out.extend_from_slice(&self.timestamp_diff_micros.to_be_bytes());
        out.extend_from_slice(&self.wnd_size.to_be_bytes());
        out.extend_from_slice(&self.seq_nr.to_be_bytes());
        out.extend_from_slice(&self.ack_nr.to_be_bytes());
        if let Some(s) = sack {
            // Single extension, then end-of-chain marker (0)
            out.push(0);
            out.push(s.len() as u8);
            out.extend_from_slice(s);
        }
        out.extend_from_slice(payload);
        out
    }

    /// Parse a datagram body into a header and its trailing payload slice
    pub fn decode(buf: &[u8]) -> io::Result<(UtpHeader, &[u8])> {
        if buf.len() < HEADER_LEN {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "utp packet shorter than header",
            ));
        }
        if buf[0] & 0x0f != UTP_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported utp version",
            ));
        }
        let packet_type = PacketType::from_nibble(buf[0] >> 4)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "unknown utp packet type"))?;
        let connection_id = u16::from_be_bytes([buf[2], buf[3]]);
        let timestamp_micros = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]);
        let timestamp_diff_micros = u32::from_be_bytes([buf[8], buf[9], buf[10], buf[11]]);
        let wnd_size = u32::from_be_bytes([buf[12], buf[13], buf[14], buf[15]]);
        let seq_nr = u16::from_be_bytes([buf[16], buf[17]]);
        let ack_nr = u16::from_be_bytes([buf[18], buf[19]]);

        // Walk the extension chain; `ext` names the type of the record at `off` and a value of 0 terminates the list
        let mut ext = buf[1];
        let mut off = HEADER_LEN;
        let mut selective_ack = None;
        while ext != 0 {
            if off + 2 > buf.len() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "truncated utp extension record",
                ));
            }
            let next_ext = buf[off];
            let len = buf[off + 1] as usize;
            off += 2;
            if off + len > buf.len() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "truncated utp extension data",
                ));
            }
            if ext == EXT_SELECTIVE_ACK {
                selective_ack = Some(buf[off..off + len].to_vec());
            }
            off += len;
            ext = next_ext;
        }

        Ok((
            UtpHeader {
                packet_type,
                connection_id,
                timestamp_micros,
                timestamp_diff_micros,
                wnd_size,
                seq_nr,
                ack_nr,
                selective_ack,
            },
            &buf[off..],
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(packet_type: PacketType) -> UtpHeader {
        UtpHeader {
            packet_type,
            connection_id: 0xABCD,
            timestamp_micros: 0x11223344,
            timestamp_diff_micros: 0x55667788,
            wnd_size: 0x0010_0000,
            seq_nr: 0x0102,
            ack_nr: 0x0304,
            selective_ack: None,
        }
    }

    #[test]
    fn data_packet_round_trips_with_payload() {
        let h = sample(PacketType::Data);
        let payload = b"hello utp";
        let bytes = h.encode(payload);
        assert_eq!(bytes.len(), HEADER_LEN + payload.len());
        // First byte: type in high nibble, version in low nibble
        assert_eq!(bytes[0], (PacketType::Data as u8) << 4 | UTP_VERSION);
        assert_eq!(bytes[1], 0); // no extension
        let (decoded, rest) = UtpHeader::decode(&bytes).unwrap();
        assert_eq!(decoded, h);
        assert_eq!(rest, payload);
    }

    #[test]
    fn every_packet_type_round_trips() {
        for ty in [
            PacketType::Data,
            PacketType::Fin,
            PacketType::State,
            PacketType::Reset,
            PacketType::Syn,
        ] {
            let h = sample(ty);
            let bytes = h.encode(&[]);
            let (decoded, rest) = UtpHeader::decode(&bytes).unwrap();
            assert_eq!(decoded.packet_type, ty);
            assert!(rest.is_empty());
        }
    }

    #[test]
    fn selective_ack_extension_round_trips() {
        let mut h = sample(PacketType::State);
        h.selective_ack = Some(vec![0b1010_0101, 0x00, 0x00, 0xFF]);
        let bytes = h.encode(&[]);
        assert_eq!(bytes[1], EXT_SELECTIVE_ACK);
        // Extension record: next_ext=0, len=4, then 4 bytes of mask
        assert_eq!(bytes[HEADER_LEN], 0);
        assert_eq!(bytes[HEADER_LEN + 1], 4);
        let (decoded, rest) = UtpHeader::decode(&bytes).unwrap();
        assert_eq!(decoded.selective_ack, h.selective_ack);
        assert!(rest.is_empty());
    }

    #[test]
    fn payload_after_extension_is_recovered() {
        let mut h = sample(PacketType::Data);
        h.selective_ack = Some(vec![0xFF, 0xFF, 0xFF, 0xFF]);
        let payload = b"data-after-sack";
        let bytes = h.encode(payload);
        let (decoded, rest) = UtpHeader::decode(&bytes).unwrap();
        assert_eq!(decoded, h);
        assert_eq!(rest, payload);
    }

    #[test]
    fn decode_rejects_short_buffer() {
        assert!(UtpHeader::decode(&[0u8; HEADER_LEN - 1]).is_err());
    }

    #[test]
    fn decode_rejects_bad_version() {
        let mut bytes = sample(PacketType::Data).encode(&[]);
        bytes[0] = (PacketType::Data as u8) << 4 | 2; // version 2
        assert!(UtpHeader::decode(&bytes).is_err());
    }

    #[test]
    fn decode_rejects_truncated_extension() {
        let mut bytes = sample(PacketType::State).encode(&[]);
        bytes[1] = EXT_SELECTIVE_ACK; // claim an extension that isn't there
        assert!(UtpHeader::decode(&bytes).is_err());
    }
}
