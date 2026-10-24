//! `paint/document.bin`: Painter's self-describing object stream, decoded
//! into a JSON tree (objects carry their type name in "@type").
//!
//! Worked out from projects the user saved (see docs/spp-format.md):
//! - header: u32 magic 0x1B7C2FDD, u32 encoding (0 = inline, 1 = registry),
//!   u32 document version;
//! - inline encoding (document versions up to 78): every value is a u8 tag
//!   then its payload, objects and lists carry their byte size;
//! - registry encoding (version 82): each object type is declared once (its
//!   field names and type codes), values carry no tags.

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};
use std::rc::Rc;

pub const MAGIC: u32 = 0x1B7C_2FDD;

/// Decodes a document stream; returns (document version, tree).
pub fn decode(b: &[u8]) -> Result<(u32, Value)> {
    let word = |i: usize| -> Result<u32> {
        Ok(u32::from_le_bytes(
            b.get(i..i + 4)
                .context("document is too short")?
                .try_into()?,
        ))
    };
    if word(0)? != MAGIC {
        bail!("not a Painter document stream");
    }
    let (encoding, version) = (word(4)?, word(8)?);
    let mut r = Reader {
        b,
        pos: 12,
        types: Vec::new(),
    };
    let tree = match encoding {
        0 => r.value()?,
        1 => {
            let root = r.u32()?;
            let declared = r.u32()? as usize;
            let tree = r.typed(root as u8)?;
            if r.pos != b.len() || r.types.len() != declared {
                bail!(
                    "document doesn't end where it should (byte {} of {})",
                    r.pos,
                    b.len()
                );
            }
            tree
        }
        e => bail!("unknown Painter document encoding {e} (version {version})"),
    };
    Ok((version, tree))
}

/// A registry-encoded type: name and (field name, type code) pairs.
type TypeDecl = (String, Vec<(String, u8)>);

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
    /// Registry encoding: types declared so far.
    types: Vec<Rc<TypeDecl>>,
}

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N]> {
        let s = self
            .b
            .get(self.pos..self.pos + N)
            .with_context(|| format!("document ends early at byte {}", self.pos))?;
        self.pos += N;
        Ok(s.try_into()?)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take::<1>()?[0])
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take()?))
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take()?))
    }

    fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.take()?))
    }

    fn floats(&mut self, n: usize) -> Result<Value> {
        let v: Vec<f32> = (0..n).map(|_| self.f32()).collect::<Result<_>>()?;
        Ok(json!(v))
    }

    fn string(&mut self) -> Result<String> {
        let n = self.u32()? as usize;
        let s = self
            .b
            .get(self.pos..self.pos + n)
            .with_context(|| format!("string runs past the end at byte {}", self.pos))?;
        self.pos += n;
        Ok(String::from_utf8_lossy(s).into_owned())
    }

    fn object(&mut self) -> Result<Value> {
        if self.u8()? == 0xFF {
            return Ok(Value::Null);
        }
        let _size = self.u32()?;
        let mut m = Map::new();
        m.insert("@type".into(), Value::String(self.string()?));
        for _ in 0..self.u16()? {
            let name = self.string()?;
            let v = self.value()?;
            m.insert(name, v);
        }
        Ok(Value::Object(m))
    }

    /// Inline encoding: a tagged value.
    fn value(&mut self) -> Result<Value> {
        let at = self.pos;
        Ok(match self.u8()? {
            0x12 | 0x14 => self.object()?,
            0x11 | 0x13 => {
                let _size = self.u32()?;
                let n = self.u32()?;
                Value::Array((0..n).map(|_| self.value()).collect::<Result<_>>()?)
            }
            0x09 => json!(i64::from_le_bytes(self.take()?)),
            t => self.scalar(t).with_context(|| format!("at byte {at}"))?,
        })
    }

    /// Registry encoding: a type reference (-1 declares a new type).
    fn type_ref(&mut self) -> Result<Rc<TypeDecl>> {
        let t = self.u32()? as i32;
        if t != -1 {
            return self
                .types
                .get(t as usize)
                .cloned()
                .with_context(|| format!("unknown type index {t} at byte {}", self.pos - 4));
        }
        let name = self.string()?;
        let n = self.u32()?;
        let fields = (0..n)
            .map(|_| Ok((self.string()?, self.u32()? as u8)))
            .collect::<Result<_>>()?;
        let decl = Rc::new((name, fields));
        self.types.push(decl.clone());
        Ok(decl)
    }

    fn fields(&mut self, t: &TypeDecl) -> Result<Value> {
        let mut m = Map::new();
        m.insert("@type".into(), Value::String(t.0.clone()));
        for (name, code) in &t.1 {
            let v = self.typed(*code)?;
            m.insert(name.clone(), v);
        }
        Ok(Value::Object(m))
    }

    /// Registry encoding: a value of type `code` (no tag in the data).
    fn typed(&mut self, code: u8) -> Result<Value> {
        Ok(match code {
            0x12 | 0x14 => {
                // Absolute offset just past the object; 0 = null.
                let end = self.u32()? as usize;
                if end == 0 {
                    return Ok(Value::Null);
                }
                let t = self.type_ref()?;
                let v = self.fields(&t)?;
                if self.pos != end {
                    bail!("{} ends at byte {}, expected {end}", t.0, self.pos);
                }
                v
            }
            // Objects of any type, each with its own header.
            0x13 => {
                let n = self.u32()?;
                Value::Array((0..n).map(|_| self.typed(0x12)).collect::<Result<_>>()?)
            }
            // Records of one type, packed.
            0x11 => {
                let n = self.u32()?;
                if n == 0 {
                    return Ok(Value::Array(Vec::new()));
                }
                let t = self.type_ref()?;
                Value::Array((0..n).map(|_| self.fields(&t)).collect::<Result<_>>()?)
            }
            0x09 => json!(self.u32()? as i32),
            c => self.scalar(c)?,
        })
    }

    /// Values sized the same in both encodings.
    fn scalar(&mut self, t: u8) -> Result<Value> {
        Ok(match t {
            0x10 => Value::String(self.string()?),
            0x01 => json!(self.f32()?),
            0x02 => self.floats(2)?,
            0x03 => self.floats(3)?,
            0x04 => self.floats(4)?,
            0x0d => self.floats(9)?,
            0x0e => self.floats(16)?,
            0x05 => json!(self.u32()? as i32),
            0x06 => json!([self.u32()? as i32, self.u32()? as i32]),
            0x0a => json!(self.u8()?),
            0x0b => json!(self.u32()?),
            0x0f | 0x15 => json!(i64::from_le_bytes(self.take()?)),
            0x0c => json!(u64::from_le_bytes(self.take()?)),
            t => bail!("unknown document value type 0x{t:02X}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds an inline-encoded stream by hand.
    struct W(Vec<u8>);

    impl W {
        fn str(&mut self, s: &str) {
            self.0.extend((s.len() as u32).to_le_bytes());
            self.0.extend(s.as_bytes());
        }
    }

    #[test]
    fn decodes_objects_lists_and_scalars() {
        let mut w = W(Vec::new());
        w.0.extend(MAGIC.to_le_bytes());
        w.0.extend(0u32.to_le_bytes());
        w.0.extend(78u32.to_le_bytes());
        w.0.extend([0x12, 0x00]);
        w.0.extend(0u32.to_le_bytes());
        w.str("DataLayer");
        w.0.extend(3u16.to_le_bytes());
        w.str("label");
        w.0.push(0x10);
        w.str("Rust");
        w.str("opacity");
        w.0.push(0x01);
        w.0.extend(0.5f32.to_le_bytes());
        w.str("items");
        w.0.push(0x13);
        w.0.extend(0u32.to_le_bytes());
        w.0.extend(2u32.to_le_bytes());
        w.0.push(0x09);
        w.0.extend((-3i64).to_le_bytes());
        w.0.extend([0x12, 0xFF]);
        let (v, tree) = decode(&w.0).unwrap();
        assert_eq!(v, 78);
        assert_eq!(
            tree,
            json!({"@type": "DataLayer", "label": "Rust", "opacity": 0.5, "items": [-3, null]})
        );
    }

    #[test]
    fn decodes_the_registry_encoding() {
        let mut w = W(Vec::new());
        w.0.extend(MAGIC.to_le_bytes());
        w.0.extend(1u32.to_le_bytes());
        w.0.extend(82u32.to_le_bytes());
        w.0.extend(0x12u32.to_le_bytes());
        w.0.extend(2u32.to_le_bytes()); // types declared
        // Root object: end offset (patched below), then a declaration.
        let end_at = w.0.len();
        w.0.extend(0u32.to_le_bytes());
        w.0.extend((-1i32).to_le_bytes());
        w.str("DataStroke");
        w.0.extend(2u32.to_le_bytes());
        w.str("mode");
        w.0.extend(0x09u32.to_le_bytes());
        w.str("points");
        w.0.extend(0x11u32.to_le_bytes());
        w.0.extend(7i32.to_le_bytes()); // mode
        w.0.extend(2u32.to_le_bytes()); // two packed keys of a new type
        w.0.extend((-1i32).to_le_bytes());
        w.str("DataStrokeKey");
        w.0.extend(1u32.to_le_bytes());
        w.str("position");
        w.0.extend(0x02u32.to_le_bytes());
        for v in [0.25f32, 0.5, 0.75, 1.0] {
            w.0.extend(v.to_le_bytes());
        }
        let end = w.0.len() as u32;
        w.0[end_at..end_at + 4].copy_from_slice(&end.to_le_bytes());
        let (v, tree) = decode(&w.0).unwrap();
        assert_eq!(v, 82);
        assert_eq!(
            tree,
            json!({"@type": "DataStroke", "mode": 7, "points": [
                {"@type": "DataStrokeKey", "position": [0.25, 0.5]},
                {"@type": "DataStrokeKey", "position": [0.75, 1.0]},
            ]})
        );
    }
}
