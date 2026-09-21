use alloc::{borrow::ToOwned, string::String, vec::Vec};

pub const SECTION: &str = "telora.snapshot";
const MAGIC: [u8; 8] = *b"TLSART01";
const VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq)]
pub enum GlobalValue {
    I32(i32),
    I64(i64),
    F32(u32),
    F64(u64),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub guest: Vec<u8>,
    pub globals: Vec<(String, GlobalValue)>,
}

fn word(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn bytes(out: &mut Vec<u8>, value: &[u8]) -> Result<(), String> {
    word(
        out,
        u32::try_from(value.len()).map_err(|_| "snapshot artifact field exceeds u32")?,
    );
    out.extend_from_slice(value);
    Ok(())
}

pub fn encode(snapshot: &Snapshot) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC);
    word(&mut out, VERSION);
    bytes(&mut out, &snapshot.guest)?;
    word(
        &mut out,
        u32::try_from(snapshot.globals.len()).map_err(|_| "too many snapshot globals")?,
    );
    for (name, value) in &snapshot.globals {
        bytes(&mut out, name.as_bytes())?;
        match value {
            GlobalValue::I32(value) => {
                out.push(0);
                out.extend_from_slice(&value.to_le_bytes());
            }
            GlobalValue::I64(value) => {
                out.push(1);
                out.extend_from_slice(&value.to_le_bytes());
            }
            GlobalValue::F32(value) => {
                out.push(2);
                out.extend_from_slice(&value.to_le_bytes());
            }
            GlobalValue::F64(value) => {
                out.push(3);
                out.extend_from_slice(&value.to_le_bytes());
            }
        }
    }
    Ok(out)
}

struct Reader<'a> {
    input: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], String> {
        let end = self
            .at
            .checked_add(len)
            .ok_or("snapshot artifact offset overflow")?;
        let value = self
            .input
            .get(self.at..end)
            .ok_or("truncated snapshot artifact")?;
        self.at = end;
        Ok(value)
    }

    fn word(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn bytes(&mut self) -> Result<&'a [u8], String> {
        let len = usize::try_from(self.word()?).unwrap();
        self.take(len)
    }
}

pub fn decode(input: &[u8]) -> Result<Snapshot, String> {
    let mut reader = Reader { input, at: 0 };
    if reader.take(MAGIC.len())? != MAGIC {
        return Err("invalid snapshot artifact magic".into());
    }
    if reader.word()? != VERSION {
        return Err("unsupported snapshot artifact version".into());
    }
    let guest = reader.bytes()?.to_vec();
    let count = usize::try_from(reader.word()?).unwrap();
    if count > input.len().saturating_sub(reader.at) / 9 {
        return Err("invalid snapshot global count".into());
    }
    let mut globals = Vec::with_capacity(count);
    for _ in 0..count {
        let name = core::str::from_utf8(reader.bytes()?)
            .map_err(|_| "snapshot global name is not UTF-8")?
            .to_owned();
        let tag = reader.take(1)?[0];
        let value = match tag {
            0 => GlobalValue::I32(i32::from_le_bytes(reader.take(4)?.try_into().unwrap())),
            1 => GlobalValue::I64(i64::from_le_bytes(reader.take(8)?.try_into().unwrap())),
            2 => GlobalValue::F32(u32::from_le_bytes(reader.take(4)?.try_into().unwrap())),
            3 => GlobalValue::F64(u64::from_le_bytes(reader.take(8)?.try_into().unwrap())),
            _ => return Err("invalid snapshot global type".into()),
        };
        globals.push((name, value));
    }
    if reader.at != input.len() {
        return Err("trailing snapshot artifact bytes".into());
    }
    Ok(Snapshot { guest, globals })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn round_trips_snapshot_and_globals() {
        let snapshot = Snapshot {
            guest: vec![0, 1, 2, 255],
            globals: vec![
                ("a".into(), GlobalValue::I32(-7)),
                ("b".into(), GlobalValue::I64(i64::MIN)),
                ("c".into(), GlobalValue::F32(f32::NAN.to_bits())),
                ("d".into(), GlobalValue::F64(f64::INFINITY.to_bits())),
            ],
        };
        assert_eq!(decode(&encode(&snapshot).unwrap()).unwrap(), snapshot);
    }

    #[test]
    fn rejects_truncation_and_trailing_bytes() {
        let bytes = encode(&Snapshot {
            guest: vec![1],
            globals: vec![],
        })
        .unwrap();
        assert!(decode(&bytes[..bytes.len() - 1]).is_err());
        let mut trailing = bytes;
        trailing.push(0);
        assert!(decode(&trailing).is_err());
    }
}
