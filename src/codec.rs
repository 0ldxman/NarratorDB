use std::collections::HashMap;
use crate::types::*;

#[derive(Debug)]
pub enum CodecError {
    UnexpectedEof,
    UnknownType(u8),
    InvalidUtf8,
}

impl std::fmt::Display for CodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CodecError::UnexpectedEof => write!(f, "unexpected end of buffer"),
            CodecError::UnknownType(t) => write!(f, "unknown type byte: 0x{:02X}", t),
            CodecError::InvalidUtf8 => write!(f, "invalid UTF-8 in text field"),
        }
    }
}

pub fn encode(value: &Value, buf: &mut Vec<u8>) {
    match value {
        Value::Null => buf.push(TYPE_NULL),
        Value::Tombstone => buf.push(TYPE_TOMBSTONE),
        Value::Bool(b) => {
            buf.push(TYPE_BOOL);
            buf.push(if *b { 1 } else { 0 });
        }
        Value::Int(i) => {
            buf.push(TYPE_INT);
            buf.extend_from_slice(&i.to_le_bytes());
        }
        Value::Float(f) => {
            buf.push(TYPE_FLOAT);
            buf.extend_from_slice(&f.to_le_bytes());
        }
        Value::Text(s) => {
            buf.push(TYPE_TEXT);
            encode_bytes(s.as_bytes(), buf);
        }
        Value::Blob(b) => {
            buf.push(TYPE_BLOB);
            encode_bytes(b, buf);
        }
        Value::Date(d) => {
            buf.push(TYPE_DATE);
            buf.extend_from_slice(&d.to_le_bytes());
        }
        Value::Time(t) => {
            buf.push(TYPE_TIME);
            buf.extend_from_slice(&t.to_le_bytes());
        }
        Value::DateTime(dt) => {
            buf.push(TYPE_DATETIME);
            buf.extend_from_slice(&dt.to_le_bytes());
        }
        Value::Array(arr) => {
            buf.push(TYPE_ARRAY);
            buf.extend_from_slice(&(arr.len() as u32).to_le_bytes());
            for item in arr {
                encode(item, buf);
            }
        }
        Value::Map(map) => {
            buf.push(TYPE_MAP);
            encode_map(map, buf);
        }
        Value::Link(link) => {
            buf.push(TYPE_LINK);
            encode_bytes(link.path.as_bytes(), buf);
            encode_map(&link.local, buf);
        }
    }
}

pub fn decode(buf: &[u8], pos: &mut usize) -> Result<Value, CodecError> {
    let type_byte = read_u8(buf, pos)?;
    match type_byte {
        TYPE_NULL => Ok(Value::Null),
        TYPE_TOMBSTONE => Ok(Value::Tombstone),
        TYPE_BOOL => {
            let b = read_u8(buf, pos)?;
            Ok(Value::Bool(b != 0))
        }
        TYPE_INT => {
            let bytes = read_bytes_fixed::<8>(buf, pos)?;
            Ok(Value::Int(i64::from_le_bytes(bytes)))
        }
        TYPE_FLOAT => {
            let bytes = read_bytes_fixed::<8>(buf, pos)?;
            Ok(Value::Float(f64::from_le_bytes(bytes)))
        }
        TYPE_TEXT => {
            let bytes = read_len_prefixed(buf, pos)?;
            let s = String::from_utf8(bytes).map_err(|_| CodecError::InvalidUtf8)?;
            Ok(Value::Text(s))
        }
        TYPE_BLOB => {
            let bytes = read_len_prefixed(buf, pos)?;
            Ok(Value::Blob(bytes))
        }
        TYPE_DATE => {
            let bytes = read_bytes_fixed::<4>(buf, pos)?;
            Ok(Value::Date(u32::from_le_bytes(bytes)))
        }
        TYPE_TIME => {
            let bytes = read_bytes_fixed::<4>(buf, pos)?;
            Ok(Value::Time(u32::from_le_bytes(bytes)))
        }
        TYPE_DATETIME => {
            let bytes = read_bytes_fixed::<8>(buf, pos)?;
            Ok(Value::DateTime(u64::from_le_bytes(bytes)))
        }
        TYPE_ARRAY => {
            let count = read_u32(buf, pos)? as usize;
            let mut arr = Vec::with_capacity(count);
            for _ in 0..count {
                arr.push(decode(buf, pos)?);
            }
            Ok(Value::Array(arr))
        }
        TYPE_MAP => {
            let map = decode_map(buf, pos)?;
            Ok(Value::Map(map))
        }
        TYPE_LINK => {
            let path_bytes = read_len_prefixed(buf, pos)?;
            let path = String::from_utf8(path_bytes).map_err(|_| CodecError::InvalidUtf8)?;
            let local = decode_map(buf, pos)?;
            Ok(Value::Link(Link { path, local }))
        }
        other => Err(CodecError::UnknownType(other)),
    }
}

fn encode_bytes(data: &[u8], buf: &mut Vec<u8>) {
    buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
    buf.extend_from_slice(data);
}

fn encode_map(map: &HashMap<String, Value>, buf: &mut Vec<u8>) {
    buf.extend_from_slice(&(map.len() as u32).to_le_bytes());
    for (key, val) in map {
        encode_bytes(key.as_bytes(), buf);
        encode(val, buf);
    }
}

fn decode_map(buf: &[u8], pos: &mut usize) -> Result<HashMap<String, Value>, CodecError> {
    let count = read_u32(buf, pos)? as usize;
    let mut map = HashMap::with_capacity(count);
    for _ in 0..count {
        let key_bytes = read_len_prefixed(buf, pos)?;
        let key = String::from_utf8(key_bytes).map_err(|_| CodecError::InvalidUtf8)?;
        let val = decode(buf, pos)?;
        map.insert(key, val);
    }
    Ok(map)
}

fn read_u8(buf: &[u8], pos: &mut usize) -> Result<u8, CodecError> {
    if *pos >= buf.len() {
        return Err(CodecError::UnexpectedEof);
    }
    let b = buf[*pos];
    *pos += 1;
    Ok(b)
}

fn read_u32(buf: &[u8], pos: &mut usize) -> Result<u32, CodecError> {
    let bytes = read_bytes_fixed::<4>(buf, pos)?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_bytes_fixed<const N: usize>(buf: &[u8], pos: &mut usize) -> Result<[u8; N], CodecError> {
    if *pos + N > buf.len() {
        return Err(CodecError::UnexpectedEof);
    }
    let mut out = [0u8; N];
    out.copy_from_slice(&buf[*pos..*pos + N]);
    *pos += N;
    Ok(out)
}

fn read_len_prefixed(buf: &[u8], pos: &mut usize) -> Result<Vec<u8>, CodecError> {
    let len = read_u32(buf, pos)? as usize;
    if *pos + len > buf.len() {
        return Err(CodecError::UnexpectedEof);
    }
    let data = buf[*pos..*pos + len].to_vec();
    *pos += len;
    Ok(data)
}
