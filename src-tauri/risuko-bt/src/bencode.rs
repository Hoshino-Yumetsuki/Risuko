//! Bencode codec (BEP-3)
//!
//! Provides:
//! - [`Value`] — an owned/borrowed bencode value tree
//! - [`decode`] / [`decode_all`] — streaming decoder that also records the
//!   byte span of every decoded value; this lets callers recover the exact
//!   raw bytes of a sub-value (needed to hash a torrent's `info` dict)
//! - [`encode_to_vec`] / [`encode_to_writer`] — canonical encoder that sorts
//!   dict keys lexicographically
//!
//! The codec intentionally operates on byte slices only; text decoding is a
//! higher layer concern

use std::collections::BTreeMap;
use std::io::{self, Write};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unexpected end of input at byte {0}")]
    UnexpectedEof(usize),
    #[error("unexpected byte {byte:#x} at position {pos}")]
    UnexpectedByte { byte: u8, pos: usize },
    #[error("integer parse error at position {pos}: {reason}")]
    BadInteger { pos: usize, reason: &'static str },
    #[error("byte string length parse error at position {pos}: {reason}")]
    BadLength { pos: usize, reason: &'static str },
    #[error("byte string too long at position {pos}: len={len}")]
    StringTooLong { pos: usize, len: u64 },
    #[error("dict keys out of order or duplicated at position {0}")]
    BadDictOrder(usize),
    #[error("dict key is not a byte string at position {0}")]
    NonStringDictKey(usize),
    #[error("trailing bytes after decoded value")]
    TrailingBytes,
    #[error("io error: {0}")]
    Io(#[from] io::Error),
}

/// A bencode value
///
/// Variants hold owned data; borrow-friendly decoding can be layered on top
/// by holding `&Value` and byte spans
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Int(i64),
    Bytes(Vec<u8>),
    List(Vec<Value>),
    /// Dict preserves insertion order for round-tripping convenience; the
    /// encoder always sorts on serialize so canonical output is guaranteed
    Dict(Vec<(Vec<u8>, Value)>),
}

impl Value {
    pub fn as_int(&self) -> Option<i64> {
        if let Value::Int(v) = self {
            Some(*v)
        } else {
            None
        }
    }
    pub fn as_bytes(&self) -> Option<&[u8]> {
        if let Value::Bytes(v) = self {
            Some(v)
        } else {
            None
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        self.as_bytes().and_then(|b| std::str::from_utf8(b).ok())
    }
    pub fn as_list(&self) -> Option<&[Value]> {
        if let Value::List(v) = self {
            Some(v)
        } else {
            None
        }
    }
    pub fn as_dict(&self) -> Option<&[(Vec<u8>, Value)]> {
        if let Value::Dict(v) = self {
            Some(v)
        } else {
            None
        }
    }
    pub fn get(&self, key: &[u8]) -> Option<&Value> {
        self.as_dict()
            .and_then(|items| items.iter().find(|(k, _)| k == key).map(|(_, v)| v))
    }
}

/// Decoded value paired with the raw byte span it was parsed from
#[derive(Debug, Clone)]
pub struct Decoded {
    pub value: Value,
    /// Half-open byte range within the original input
    pub span: std::ops::Range<usize>,
}

/// Decode a single bencode value at the start of `input`
///
/// Returns the decoded value and the number of bytes consumed
pub fn decode(input: &[u8]) -> Result<Decoded, Error> {
    let mut p = Parser { buf: input, pos: 0 };
    let v = p.parse_value()?;
    Ok(v)
}

/// Decode a single value and error if trailing bytes remain.
pub fn decode_all(input: &[u8]) -> Result<Value, Error> {
    let d = decode(input)?;
    if d.span.end != input.len() {
        return Err(Error::TrailingBytes);
    }
    Ok(d.value)
}

struct Parser<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Result<u8, Error> {
        self.buf
            .get(self.pos)
            .copied()
            .ok_or(Error::UnexpectedEof(self.pos))
    }

    fn parse_value(&mut self) -> Result<Decoded, Error> {
        let start = self.pos;
        let b = self.peek()?;
        let value = match b {
            b'i' => self.parse_int()?,
            b'l' => self.parse_list()?,
            b'd' => self.parse_dict()?,
            b'0'..=b'9' => self.parse_bytes()?,
            other => {
                return Err(Error::UnexpectedByte {
                    byte: other,
                    pos: self.pos,
                });
            }
        };
        Ok(Decoded {
            value,
            span: start..self.pos,
        })
    }

    fn parse_int(&mut self) -> Result<Value, Error> {
        debug_assert_eq!(self.buf[self.pos], b'i');
        self.pos += 1;
        let start = self.pos;
        while self.peek()? != b'e' {
            self.pos += 1;
        }
        let digits = &self.buf[start..self.pos];
        // skip 'e'
        self.pos += 1;
        // Reject "i-0e" and leading zeros like "i01e" / "i-01e" per BEP-3
        if digits.is_empty() {
            return Err(Error::BadInteger {
                pos: start,
                reason: "empty",
            });
        }
        if digits[0] == b'+' {
            return Err(Error::BadInteger {
                pos: start,
                reason: "leading plus",
            });
        }
        if digits == b"-0" {
            return Err(Error::BadInteger {
                pos: start,
                reason: "negative zero",
            });
        }
        if digits.len() > 1 {
            let leading = if digits[0] == b'-' {
                &digits[1..]
            } else {
                digits
            };
            if leading.starts_with(b"0") {
                return Err(Error::BadInteger {
                    pos: start,
                    reason: "leading zero",
                });
            }
        }
        let s = std::str::from_utf8(digits).map_err(|_| Error::BadInteger {
            pos: start,
            reason: "non-ascii",
        })?;
        let n: i64 = s.parse().map_err(|_| Error::BadInteger {
            pos: start,
            reason: "out of range",
        })?;
        Ok(Value::Int(n))
    }

    fn parse_bytes(&mut self) -> Result<Value, Error> {
        let start = self.pos;
        while self.peek()? != b':' {
            self.pos += 1;
        }
        let digits = &self.buf[start..self.pos];
        // skip ':'
        self.pos += 1;
        let s = std::str::from_utf8(digits).map_err(|_| Error::BadLength {
            pos: start,
            reason: "non-ascii",
        })?;
        let len: u64 = s.parse().map_err(|_| Error::BadLength {
            pos: start,
            reason: "parse",
        })?;
        // Guard against pathological lengths on untrusted input
        if len > (isize::MAX as u64) {
            return Err(Error::StringTooLong { pos: start, len });
        }
        let len = len as usize;
        let end = self.pos.checked_add(len).ok_or(Error::StringTooLong {
            pos: start,
            len: len as u64,
        })?;
        if end > self.buf.len() {
            return Err(Error::UnexpectedEof(self.buf.len()));
        }
        let bytes = self.buf[self.pos..end].to_vec();
        self.pos = end;
        Ok(Value::Bytes(bytes))
    }

    fn parse_list(&mut self) -> Result<Value, Error> {
        debug_assert_eq!(self.buf[self.pos], b'l');
        self.pos += 1;
        let mut items = Vec::new();
        while self.peek()? != b'e' {
            items.push(self.parse_value()?.value);
        }
        self.pos += 1;
        Ok(Value::List(items))
    }

    fn parse_dict(&mut self) -> Result<Value, Error> {
        debug_assert_eq!(self.buf[self.pos], b'd');
        self.pos += 1;
        let mut items: Vec<(Vec<u8>, Value)> = Vec::new();
        while self.peek()? != b'e' {
            let key_pos = self.pos;
            let key_val = self.parse_value()?.value;
            let key = match key_val {
                Value::Bytes(b) => b,
                _ => return Err(Error::NonStringDictKey(key_pos)),
            };
            if let Some((prev, _)) = items.last() {
                if prev.as_slice() >= key.as_slice() {
                    return Err(Error::BadDictOrder(key_pos));
                }
            }
            let value = self.parse_value()?.value;
            items.push((key, value));
        }
        self.pos += 1;
        Ok(Value::Dict(items))
    }
}

/// Look up a top-level dict value and return both the decoded value and its
/// raw byte span within `input`. Useful for hashing the `info` dict
pub fn decode_dict_field_raw<'a>(
    input: &'a [u8],
    key: &[u8],
) -> Result<Option<(Value, &'a [u8])>, Error> {
    let mut p = Parser { buf: input, pos: 0 };
    if p.peek()? != b'd' {
        return Err(Error::UnexpectedByte {
            byte: input[0],
            pos: 0,
        });
    }
    p.pos += 1;
    while p.peek()? != b'e' {
        let k = match p.parse_value()?.value {
            Value::Bytes(b) => b,
            _ => return Err(Error::NonStringDictKey(p.pos)),
        };
        let val_start = p.pos;
        let v = p.parse_value()?;
        if k == key {
            let raw = &input[val_start..v.span.end];
            return Ok(Some((v.value, raw)));
        }
    }
    Ok(None)
}

// -- Encoder --

pub fn encode_to_vec(v: &Value) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    encode_to_writer(v, &mut out).expect("Vec write is infallible");
    out
}

pub fn encode_to_writer<W: Write>(v: &Value, w: &mut W) -> Result<(), Error> {
    match v {
        Value::Int(n) => {
            write!(w, "i{n}e")?;
        }
        Value::Bytes(b) => {
            write!(w, "{}:", b.len())?;
            w.write_all(b)?;
        }
        Value::List(items) => {
            w.write_all(b"l")?;
            for item in items {
                encode_to_writer(item, w)?;
            }
            w.write_all(b"e")?;
        }
        Value::Dict(items) => {
            // Sort into a BTreeMap to enforce canonical key order. Reject
            // duplicates explicitly instead of silently dropping them.
            let mut sorted: BTreeMap<&[u8], &Value> = BTreeMap::new();
            for (k, vv) in items {
                if sorted.insert(k.as_slice(), vv).is_some() {
                    return Err(Error::BadDictOrder(0));
                }
            }
            w.write_all(b"d")?;
            for (k, vv) in sorted {
                write!(w, "{}:", k.len())?;
                w.write_all(k)?;
                encode_to_writer(vv, w)?;
            }
            w.write_all(b"e")?;
        }
    }
    Ok(())
}

// -- Tests --

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_int() {
        assert_eq!(decode_all(b"i42e").unwrap(), Value::Int(42));
        assert_eq!(decode_all(b"i-7e").unwrap(), Value::Int(-7));
        assert_eq!(decode_all(b"i0e").unwrap(), Value::Int(0));
        assert!(decode_all(b"i-0e").is_err());
        assert!(decode_all(b"i01e").is_err());
        assert!(decode_all(b"ie").is_err());
    }

    #[test]
    fn decode_bytes() {
        assert_eq!(
            decode_all(b"4:spam").unwrap(),
            Value::Bytes(b"spam".to_vec())
        );
        assert_eq!(decode_all(b"0:").unwrap(), Value::Bytes(vec![]));
        assert!(decode_all(b"5:abc").is_err());
    }

    #[test]
    fn decode_list_and_dict() {
        let v = decode_all(b"l4:spami42ee").unwrap();
        assert_eq!(
            v,
            Value::List(vec![Value::Bytes(b"spam".to_vec()), Value::Int(42)])
        );
        let d = decode_all(b"d3:bar4:spam3:fooi42ee").unwrap();
        assert_eq!(
            d,
            Value::Dict(vec![
                (b"bar".to_vec(), Value::Bytes(b"spam".to_vec())),
                (b"foo".to_vec(), Value::Int(42)),
            ])
        );
    }

    #[test]
    fn reject_unsorted_dict() {
        assert!(decode_all(b"d3:fooi1e3:bari2ee").is_err());
        assert!(decode_all(b"d3:fooi1e3:fooi2ee").is_err());
    }

    #[test]
    fn encode_round_trip_sorts_dicts() {
        let v = Value::Dict(vec![
            (b"zeta".to_vec(), Value::Int(1)),
            (b"alpha".to_vec(), Value::Int(2)),
        ]);
        let encoded = encode_to_vec(&v);
        assert_eq!(encoded, b"d5:alphai2e4:zetai1ee");
        let round = decode_all(&encoded).unwrap();
        assert_eq!(
            round,
            Value::Dict(vec![
                (b"alpha".to_vec(), Value::Int(2)),
                (b"zeta".to_vec(), Value::Int(1)),
            ])
        );
    }

    #[test]
    fn raw_field_span() {
        let doc = b"d4:infod6:pieces4:aaaae4:name4:abcde";
        let (v, raw) = decode_dict_field_raw(doc, b"info").unwrap().unwrap();
        assert!(matches!(v, Value::Dict(_)));
        assert_eq!(raw, b"d6:pieces4:aaaae");
    }

    #[test]
    fn decode_records_spans() {
        let input = b"l4:spami42ee";
        let d = decode(input).unwrap();
        assert_eq!(d.span, 0..input.len());
    }
}
