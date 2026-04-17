//! Peer wire messages (BEP-3 & BEP-10 extension container).
//!
//! Frame format: `<len: u32 BE><msg_id: u8?><payload>`. A length of 0 is a
//! keep-alive (no id, no payload)

use bytes::{Buf, BufMut, Bytes, BytesMut};

/// Practical upper bound for a single peer message. BEP-3 chunks are 16 KiB;
/// ut_metadata pieces are also 16 KiB. We allow a generous headroom
pub const MAX_MESSAGE_BYTES: usize = 2 * 1024 * 1024;

pub mod id {
    pub const CHOKE: u8 = 0;
    pub const UNCHOKE: u8 = 1;
    pub const INTERESTED: u8 = 2;
    pub const NOT_INTERESTED: u8 = 3;
    pub const HAVE: u8 = 4;
    pub const BITFIELD: u8 = 5;
    pub const REQUEST: u8 = 6;
    pub const PIECE: u8 = 7;
    pub const CANCEL: u8 = 8;
    pub const PORT: u8 = 9;
    /// BEP-10 extension protocol container.
    pub const EXTENDED: u8 = 20;
}

#[derive(Debug, thiserror::Error)]
pub enum MessageError {
    #[error("truncated message: id {id}, expected {expected} bytes, got {got}")]
    Truncated { id: u8, expected: usize, got: usize },
    #[error("unknown message id {0}")]
    UnknownId(u8),
    #[error("message too large: {0} bytes")]
    TooLarge(usize),
}

#[derive(Debug, Clone)]
pub enum Message {
    KeepAlive,
    Choke,
    Unchoke,
    Interested,
    NotInterested,
    Have {
        piece_index: u32,
    },
    Bitfield(Bytes),
    Request {
        index: u32,
        begin: u32,
        length: u32,
    },
    Piece {
        index: u32,
        begin: u32,
        data: Bytes,
    },
    Cancel {
        index: u32,
        begin: u32,
        length: u32,
    },
    Port(u16),
    Extended {
        ext_id: u8,
        payload: Bytes,
    },
    /// Fallback for message ids we don't recognise — we keep the raw payload
    /// so that future decode upgrades don't need protocol changes.
    Unknown {
        id: u8,
        payload: Bytes,
    },
}

pub struct MessageEncoder;

impl MessageEncoder {
    /// Encode a message to bytes ready to write on the wire (length prefix
    /// included).
    pub fn encode(msg: &Message) -> Bytes {
        let mut buf = BytesMut::with_capacity(64);
        // Reserve 4 bytes for the length prefix; fill after we know body size.
        buf.put_u32(0);
        match msg {
            Message::KeepAlive => {}
            Message::Choke => buf.put_u8(id::CHOKE),
            Message::Unchoke => buf.put_u8(id::UNCHOKE),
            Message::Interested => buf.put_u8(id::INTERESTED),
            Message::NotInterested => buf.put_u8(id::NOT_INTERESTED),
            Message::Have { piece_index } => {
                buf.put_u8(id::HAVE);
                buf.put_u32(*piece_index);
            }
            Message::Bitfield(b) => {
                buf.put_u8(id::BITFIELD);
                buf.extend_from_slice(b);
            }
            Message::Request {
                index,
                begin,
                length,
            } => {
                buf.put_u8(id::REQUEST);
                buf.put_u32(*index);
                buf.put_u32(*begin);
                buf.put_u32(*length);
            }
            Message::Piece { index, begin, data } => {
                buf.put_u8(id::PIECE);
                buf.put_u32(*index);
                buf.put_u32(*begin);
                buf.extend_from_slice(data);
            }
            Message::Cancel {
                index,
                begin,
                length,
            } => {
                buf.put_u8(id::CANCEL);
                buf.put_u32(*index);
                buf.put_u32(*begin);
                buf.put_u32(*length);
            }
            Message::Port(port) => {
                buf.put_u8(id::PORT);
                buf.put_u16(*port);
            }
            Message::Extended { ext_id, payload } => {
                buf.put_u8(id::EXTENDED);
                buf.put_u8(*ext_id);
                buf.extend_from_slice(payload);
            }
            Message::Unknown { id, payload } => {
                buf.put_u8(*id);
                buf.extend_from_slice(payload);
            }
        }
        let body_len = (buf.len() - 4) as u32;
        buf[..4].copy_from_slice(&body_len.to_be_bytes());
        buf.freeze()
    }
}

/// Stateful decoder that consumes bytes from a `BytesMut` buffer.
#[derive(Default)]
pub struct MessageDecoder;

impl MessageDecoder {
    /// Try to decode a single message from the buffer. Returns `Ok(None)`
    /// when more bytes are needed.
    pub fn try_decode(buf: &mut BytesMut) -> Result<Option<Message>, MessageError> {
        if buf.len() < 4 {
            return Ok(None);
        }
        let mut len_bytes = [0u8; 4];
        len_bytes.copy_from_slice(&buf[..4]);
        let body_len = u32::from_be_bytes(len_bytes) as usize;
        if body_len > MAX_MESSAGE_BYTES {
            return Err(MessageError::TooLarge(body_len));
        }
        if buf.len() < 4 + body_len {
            return Ok(None);
        }
        buf.advance(4);
        if body_len == 0 {
            return Ok(Some(Message::KeepAlive));
        }
        let id = buf.get_u8();
        let remaining = body_len - 1;
        let take = |buf: &mut BytesMut, n: usize, id: u8| -> Result<Bytes, MessageError> {
            if buf.remaining() < n {
                return Err(MessageError::Truncated {
                    id,
                    expected: n,
                    got: buf.remaining(),
                });
            }
            Ok(buf.copy_to_bytes(n))
        };
        let msg = match id {
            id::CHOKE => {
                take(buf, remaining, id)?;
                Message::Choke
            }
            id::UNCHOKE => {
                take(buf, remaining, id)?;
                Message::Unchoke
            }
            id::INTERESTED => {
                take(buf, remaining, id)?;
                Message::Interested
            }
            id::NOT_INTERESTED => {
                take(buf, remaining, id)?;
                Message::NotInterested
            }
            id::HAVE => {
                if remaining != 4 {
                    return Err(MessageError::Truncated {
                        id,
                        expected: 4,
                        got: remaining,
                    });
                }
                Message::Have {
                    piece_index: buf.get_u32(),
                }
            }
            id::BITFIELD => {
                let b = take(buf, remaining, id)?;
                Message::Bitfield(b)
            }
            id::REQUEST => {
                if remaining != 12 {
                    return Err(MessageError::Truncated {
                        id,
                        expected: 12,
                        got: remaining,
                    });
                }
                let index = buf.get_u32();
                let begin = buf.get_u32();
                let length = buf.get_u32();
                Message::Request {
                    index,
                    begin,
                    length,
                }
            }
            id::PIECE => {
                if remaining < 8 {
                    return Err(MessageError::Truncated {
                        id,
                        expected: 8,
                        got: remaining,
                    });
                }
                let index = buf.get_u32();
                let begin = buf.get_u32();
                let data = take(buf, remaining - 8, id)?;
                Message::Piece { index, begin, data }
            }
            id::CANCEL => {
                if remaining != 12 {
                    return Err(MessageError::Truncated {
                        id,
                        expected: 12,
                        got: remaining,
                    });
                }
                let index = buf.get_u32();
                let begin = buf.get_u32();
                let length = buf.get_u32();
                Message::Cancel {
                    index,
                    begin,
                    length,
                }
            }
            id::PORT => {
                if remaining != 2 {
                    return Err(MessageError::Truncated {
                        id,
                        expected: 2,
                        got: remaining,
                    });
                }
                Message::Port(buf.get_u16())
            }
            id::EXTENDED => {
                if remaining < 1 {
                    return Err(MessageError::Truncated {
                        id,
                        expected: 1,
                        got: remaining,
                    });
                }
                let ext_id = buf.get_u8();
                let payload = take(buf, remaining - 1, id)?;
                Message::Extended { ext_id, payload }
            }
            other => {
                let payload = take(buf, remaining, other)?;
                Message::Unknown { id: other, payload }
            }
        };
        Ok(Some(msg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(msg: Message) -> Message {
        let bytes = MessageEncoder::encode(&msg);
        let mut buf = BytesMut::from(&bytes[..]);
        let decoded = MessageDecoder::try_decode(&mut buf).unwrap().unwrap();
        assert!(buf.is_empty(), "decoder did not consume full frame");
        decoded
    }

    #[test]
    fn keepalive_round_trip() {
        assert!(matches!(round_trip(Message::KeepAlive), Message::KeepAlive));
    }

    #[test]
    fn request_round_trip() {
        let m = Message::Request {
            index: 5,
            begin: 16_384,
            length: 16_384,
        };
        if let Message::Request {
            index,
            begin,
            length,
        } = round_trip(m)
        {
            assert_eq!((index, begin, length), (5, 16_384, 16_384));
        } else {
            panic!();
        }
    }

    #[test]
    fn piece_round_trip() {
        let payload = Bytes::from_static(b"hello world");
        let m = Message::Piece {
            index: 0,
            begin: 0,
            data: payload.clone(),
        };
        if let Message::Piece { data, .. } = round_trip(m) {
            assert_eq!(data, payload);
        } else {
            panic!();
        }
    }

    #[test]
    fn partial_frame_yields_none() {
        let bytes = MessageEncoder::encode(&Message::Have { piece_index: 7 });
        let mut buf = BytesMut::from(&bytes[..bytes.len() - 1]);
        assert!(MessageDecoder::try_decode(&mut buf).unwrap().is_none());
    }

    #[test]
    fn unknown_id_preserved() {
        let mut raw = BytesMut::new();
        raw.put_u32(3);
        raw.put_u8(99);
        raw.put_slice(b"hi");
        let decoded = MessageDecoder::try_decode(&mut raw).unwrap().unwrap();
        match decoded {
            Message::Unknown { id, payload } => {
                assert_eq!(id, 99);
                assert_eq!(payload.as_ref(), b"hi");
            }
            _ => panic!(),
        }
    }

    #[test]
    fn rejects_oversized_frame() {
        let mut raw = BytesMut::new();
        raw.put_u32((MAX_MESSAGE_BYTES + 1) as u32);
        let err = MessageDecoder::try_decode(&mut raw).unwrap_err();
        assert!(matches!(err, MessageError::TooLarge(_)));
    }
}
