//! Row representation and the SQLite record (serial-type) byte format.
//!
//! Records use SQLite's format so `sqlite3` can read them:
//! `header = varint(header_len) ++ serial_types...`, then the value bodies.
//! See <https://www.sqlite.org/fileformat.html#record_format>.

use squrust_core::varint;

use crate::error::{Result, SqlError};
use crate::types::Value;

pub type RowId = i64;

#[derive(Debug, Clone, Default)]
pub struct Row {
    pub row_id: RowId,
    pub values: Vec<Value>,
}

impl Row {
    pub fn new(row_id: RowId, values: Vec<Value>) -> Self {
        Row { row_id, values }
    }

    /// Encode the value list into a SQLite record. The row id is the b-tree key
    /// and is stored separately by the b-tree cell.
    pub fn encode(&self) -> Vec<u8> {
        let mut serials: Vec<u64> = Vec::with_capacity(self.values.len());
        let mut body: Vec<u8> = Vec::new();
        for v in &self.values {
            serials.push(encode_value(v, &mut body));
        }

        // Header = varint(header_len) ++ serial type varints, where header_len
        // counts the size-varint itself. Solve the self-reference by iterating
        // to a fixed point (converges in at most a couple of steps).
        let serial_bytes: usize = serials.iter().map(|s| varint::len(*s)).sum();
        let mut header_len = serial_bytes + 1;
        loop {
            let candidate = serial_bytes + varint::len(header_len as u64);
            if candidate == header_len {
                break;
            }
            header_len = candidate;
        }

        let mut out = Vec::with_capacity(header_len + body.len());
        varint::write(&mut out, header_len as u64);
        for s in &serials {
            varint::write(&mut out, *s);
        }
        out.extend_from_slice(&body);
        out
    }

    /// Decode a SQLite record into a value list.
    pub fn decode(row_id: RowId, data: &[u8]) -> Result<Row> {
        let (header_len, mut pos) =
            varint::read(data).ok_or_else(|| SqlError::Type("bad record header".into()))?;
        let header_end = header_len as usize;
        let mut serials = Vec::new();
        while pos < header_end {
            let (s, n) = varint::read(&data[pos..])
                .ok_or_else(|| SqlError::Type("bad serial type".into()))?;
            serials.push(s);
            pos += n;
        }

        let mut body = header_end;
        let mut values = Vec::with_capacity(serials.len());
        for s in serials {
            let (v, used) = decode_value(s, &data[body..])?;
            values.push(v);
            body += used;
        }
        Ok(Row { row_id, values })
    }
}

/// Append the body bytes for `v` and return its serial type code.
fn encode_value(v: &Value, body: &mut Vec<u8>) -> u64 {
    match v {
        Value::Null => 0,
        Value::Integer(i) => encode_int(*i, body),
        Value::Boolean(b) => encode_int(*b as i64, body),
        Value::Real(r) => {
            body.extend_from_slice(&r.to_bits().to_be_bytes());
            7
        }
        Value::Text(s) => {
            body.extend_from_slice(s.as_bytes());
            (s.len() as u64) * 2 + 13
        }
        Value::Json(j) => {
            let s = j.to_string();
            body.extend_from_slice(s.as_bytes());
            (s.len() as u64) * 2 + 13
        }
        Value::Blob(b) => {
            body.extend_from_slice(b);
            (b.len() as u64) * 2 + 12
        }
    }
}

fn encode_int(i: i64, body: &mut Vec<u8>) -> u64 {
    match i {
        0 => 8,
        1 => 9,
        _ if (i8::MIN as i64..=i8::MAX as i64).contains(&i) => {
            body.push(i as u8);
            1
        }
        _ if (i16::MIN as i64..=i16::MAX as i64).contains(&i) => {
            body.extend_from_slice(&(i as i16).to_be_bytes());
            2
        }
        _ if (-(1 << 23)..(1 << 23)).contains(&i) => {
            body.extend_from_slice(&i.to_be_bytes()[5..8]);
            3
        }
        _ if (i32::MIN as i64..=i32::MAX as i64).contains(&i) => {
            body.extend_from_slice(&(i as i32).to_be_bytes());
            4
        }
        _ if (-(1 << 47)..(1 << 47)).contains(&i) => {
            body.extend_from_slice(&i.to_be_bytes()[2..8]);
            5
        }
        _ => {
            body.extend_from_slice(&i.to_be_bytes());
            6
        }
    }
}

fn decode_value(serial: u64, data: &[u8]) -> Result<(Value, usize)> {
    Ok(match serial {
        0 => (Value::Null, 0),
        1 => (Value::Integer(read_int(data, 1)?), 1),
        2 => (Value::Integer(read_int(data, 2)?), 2),
        3 => (Value::Integer(read_int(data, 3)?), 3),
        4 => (Value::Integer(read_int(data, 4)?), 4),
        5 => (Value::Integer(read_int(data, 6)?), 6),
        6 => (Value::Integer(read_int(data, 8)?), 8),
        7 => {
            if data.len() < 8 {
                return Err(SqlError::Type("truncated real".into()));
            }
            let bits = u64::from_be_bytes(data[..8].try_into().unwrap());
            (Value::Real(f64::from_bits(bits)), 8)
        }
        8 => (Value::Integer(0), 0),
        9 => (Value::Integer(1), 0),
        s if s >= 12 && s % 2 == 0 => {
            let n = ((s - 12) / 2) as usize;
            need(data, n)?;
            (Value::Blob(data[..n].to_vec()), n)
        }
        s if s >= 13 => {
            let n = ((s - 13) / 2) as usize;
            need(data, n)?;
            let text = String::from_utf8(data[..n].to_vec())
                .map_err(|e| SqlError::Type(format!("invalid UTF-8 text: {e}")))?;
            (Value::Text(text), n)
        }
        other => return Err(SqlError::Type(format!("reserved serial type {other}"))),
    })
}

fn read_int(data: &[u8], n: usize) -> Result<i64> {
    need(data, n)?;
    let mut v: i64 = if data[0] & 0x80 != 0 { -1 } else { 0 };
    for &b in &data[..n] {
        v = (v << 8) | (b as i64);
    }
    Ok(v)
}

fn need(data: &[u8], n: usize) -> Result<()> {
    if data.len() < n {
        Err(SqlError::Type("truncated record body".into()))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_roundtrip() {
        let row = Row::new(
            7,
            vec![
                Value::Integer(42),
                Value::Text("hello".into()),
                Value::Null,
                Value::Real(3.5),
                Value::Integer(0),
                Value::Integer(1),
                Value::Blob(vec![1, 2, 3]),
                Value::Integer(-1000),
                Value::Integer(9_000_000_000),
            ],
        );
        let bytes = row.encode();
        let back = Row::decode(7, &bytes).unwrap();
        assert_eq!(back.values[0], Value::Integer(42));
        assert_eq!(back.values[1], Value::Text("hello".into()));
        assert!(back.values[2].is_null());
        assert_eq!(back.values[3], Value::Real(3.5));
        assert_eq!(back.values[4], Value::Integer(0));
        assert_eq!(back.values[5], Value::Integer(1));
        assert_eq!(back.values[6], Value::Blob(vec![1, 2, 3]));
        assert_eq!(back.values[7], Value::Integer(-1000));
        assert_eq!(back.values[8], Value::Integer(9_000_000_000));
        assert_eq!(back.row_id, 7);
    }

    #[test]
    fn empty_record() {
        let row = Row::new(1, vec![]);
        let bytes = row.encode();
        let back = Row::decode(1, &bytes).unwrap();
        assert!(back.values.is_empty());
    }
}
