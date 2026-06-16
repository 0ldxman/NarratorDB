use std::collections::HashMap;

pub type Map = HashMap<String, Value>;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Tombstone,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    Blob(Vec<u8>),
    Date(u32),
    Time(u32),
    DateTime(u64),
    Array(Vec<Value>),
    Map(Map),
    Link(Link),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub path: String,
    pub local: Map,
}

impl Link {
    pub fn simple(path: impl Into<String>) -> Self {
        Self { path: path.into(), local: HashMap::new() }
    }

    pub fn with_local(path: impl Into<String>, local: Map) -> Self {
        Self { path: path.into(), local }
    }
}

pub const TYPE_NULL: u8      = 0x00;
pub const TYPE_BOOL: u8      = 0x01;
pub const TYPE_INT: u8       = 0x02;
pub const TYPE_FLOAT: u8     = 0x03;
pub const TYPE_TEXT: u8      = 0x04;
pub const TYPE_LINK: u8      = 0x05;
pub const TYPE_BLOB: u8      = 0x06;
pub const TYPE_ARRAY: u8     = 0x07;
pub const TYPE_MAP: u8       = 0x08;
pub const TYPE_DATE: u8      = 0x09;
pub const TYPE_TIME: u8      = 0x0A;
pub const TYPE_DATETIME: u8  = 0x0B;
pub const TYPE_TOMBSTONE: u8 = 0x0C;
