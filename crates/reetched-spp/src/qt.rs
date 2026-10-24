//! Qt `QDataStream` values (big-endian): the `QVariantMap`s Painter writes for
//! image headers, resource metadata and texture cache entries. The encoding
//! is Qt's documented serialization format.

use anyhow::{Result, bail};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub enum QValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Bytes(Vec<u8>),
    List(Vec<QValue>),
    Map(BTreeMap<String, QValue>),
}

impl QValue {
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            QValue::Int(v) => Some(*v),
            QValue::Float(v) => Some(*v as i64),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            QValue::String(s) => Some(s),
            _ => None,
        }
    }
}

pub type QMap = BTreeMap<String, QValue>;

pub struct Reader<'a> {
    b: &'a [u8],
    pub pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        Self { b, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let Some(s) = self.b.get(self.pos..self.pos + n) else {
            bail!("Qt data ends early at byte {}", self.pos);
        };
        self.pos += n;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into()?))
    }

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into()?))
    }

    fn string(&mut self) -> Result<Option<String>> {
        let n = self.u32()?;
        if n == u32::MAX {
            return Ok(None);
        }
        let units: Vec<u16> = self
            .take(n as usize)?
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        Ok(Some(String::from_utf16_lossy(&units)))
    }

    fn bytes(&mut self) -> Result<Vec<u8>> {
        let n = self.u32()?;
        if n == u32::MAX {
            return Ok(Vec::new());
        }
        Ok(self.take(n as usize)?.to_vec())
    }

    /// A `QVariant`: type id, null flag, value (Qt 5 type ids).
    pub fn variant(&mut self) -> Result<QValue> {
        let t = self.u32()?;
        let _null = self.u8()?;
        Ok(match t {
            0 => QValue::Null,
            1 => QValue::Bool(self.u8()? != 0),
            2 => QValue::Int(self.u32()? as i32 as i64),
            3 => QValue::Int(self.u32()? as i64),
            4 => QValue::Int(self.u64()? as i64),
            5 => QValue::Int(self.u64()? as i64),
            6 => QValue::Float(f64::from_bits(self.u64()?)),
            38 => QValue::Float(f32::from_bits(self.u32()?) as f64),
            10 => QValue::String(self.string()?.unwrap_or_default()),
            12 => QValue::Bytes(self.bytes()?),
            8 => QValue::Map(self.map()?),
            9 => {
                let n = self.u32()?;
                QValue::List((0..n).map(|_| self.variant()).collect::<Result<_>>()?)
            }
            11 => {
                let n = self.u32()?;
                QValue::List(
                    (0..n)
                        .map(|_| Ok(QValue::String(self.string()?.unwrap_or_default())))
                        .collect::<Result<_>>()?,
                )
            }
            t => bail!("unsupported Qt value type {t} at byte {}", self.pos - 5),
        })
    }

    /// A `QVariantMap`: count, then (string key, variant) pairs.
    pub fn map(&mut self) -> Result<QMap> {
        let n = self.u32()?;
        let mut m = QMap::new();
        for _ in 0..n {
            let k = self.string()?.unwrap_or_default();
            m.insert(k, self.variant()?);
        }
        Ok(m)
    }
}

/// Reads a map from the start of `b`; also returns where the map ended.
pub fn read_map(b: &[u8]) -> Result<(QMap, usize)> {
    let mut r = Reader::new(b);
    let m = r.map()?;
    Ok((m, r.pos))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qstr(s: &str) -> Vec<u8> {
        let units: Vec<u8> = s.encode_utf16().flat_map(|u| u.to_be_bytes()).collect();
        let mut v = (units.len() as u32).to_be_bytes().to_vec();
        v.extend(units);
        v
    }

    #[test]
    fn reads_a_map_of_ints_and_strings() {
        let mut b = 2u32.to_be_bytes().to_vec();
        b.extend(qstr("width"));
        b.extend(2u32.to_be_bytes());
        b.push(0);
        b.extend(1024u32.to_be_bytes());
        b.extend(qstr("format"));
        b.extend(10u32.to_be_bytes());
        b.push(0);
        b.extend(qstr("RGBA16"));
        b.extend([9, 9]); // trailing payload
        let (m, end) = read_map(&b).unwrap();
        assert_eq!(m["width"].as_i64(), Some(1024));
        assert_eq!(m["format"].as_str(), Some("RGBA16"));
        assert_eq!(end, b.len() - 2);
    }
}
