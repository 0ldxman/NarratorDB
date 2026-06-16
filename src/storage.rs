use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::codec::{decode, encode, CodecError};
use crate::types::Value;

// Record layout:
//   [key_len: u32 LE][key: bytes][val_len: u32 LE][val: bytes]
//
// Two file handles: reader (seekable) + writer (BufWriter, append-only).
// write_pos tracks the logical end of file without seeking.
// In batch mode BufWriter accumulates in userspace; flush on commit.
// In non-batch mode flush after every set() call.

const BUF_CAPACITY: usize = 64 * 1024; // 64 KB write buffer

#[derive(Debug)]
pub enum StorageError {
    Io(std::io::Error),
    Codec(CodecError),
    KeyNotFound,
}

impl From<std::io::Error> for StorageError {
    fn from(e: std::io::Error) -> Self { StorageError::Io(e) }
}

impl From<CodecError> for StorageError {
    fn from(e: CodecError) -> Self { StorageError::Codec(e) }
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageError::Io(e) => write!(f, "io: {}", e),
            StorageError::Codec(e) => write!(f, "codec: {}", e),
            StorageError::KeyNotFound => write!(f, "key not found"),
        }
    }
}

pub struct Collection {
    reader:    File,
    writer:    BufWriter<File>,
    write_pos: u64,
    index:     HashMap<String, u64>,
    cache:     HashMap<String, Value>,
    batching:  bool,
    path:      PathBuf,
}

impl Collection {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let path = path.as_ref().to_path_buf();

        // Create the file if it doesn't exist.
        OpenOptions::new().create(true).append(true).open(&path)?;

        let reader     = OpenOptions::new().read(true).open(&path)?;
        let write_file = OpenOptions::new().append(true).open(&path)?;
        let write_pos  = write_file.metadata()?.len();
        let writer     = BufWriter::with_capacity(BUF_CAPACITY, write_file);

        let mut col = Collection {
            reader,
            writer,
            write_pos,
            index:    HashMap::new(),
            cache:    HashMap::new(),
            batching: false,
            path,
        };
        col.build_index()?;
        Ok(col)
    }

    pub fn get(&mut self, key: &str) -> Result<Value, StorageError> {
        if let Some(val) = self.cache.get(key) {
            return Ok(val.clone());
        }
        let &offset = self.index.get(key).ok_or(StorageError::KeyNotFound)?;
        self.read_value_at(offset)
    }

    pub fn set(&mut self, key: &str, value: &Value) -> Result<(), StorageError> {
        let offset = self.append_record(key, value)?;
        match value {
            Value::Tombstone => {
                self.index.remove(key);
                self.cache.remove(key);
            }
            _ => {
                self.index.insert(key.to_string(), offset);
                self.cache.insert(key.to_string(), value.clone());
            }
        }
        if !self.batching {
            self.writer.flush()?;
        }
        Ok(())
    }

    pub fn delete(&mut self, key: &str) -> Result<(), StorageError> {
        self.set(key, &Value::Tombstone)
    }

    pub fn begin_batch(&mut self) {
        self.batching = true;
    }

    pub fn commit_batch(&mut self) -> Result<(), StorageError> {
        self.writer.flush()?;
        self.batching = false;
        Ok(())
    }

    pub fn contains(&self, key: &str) -> bool {
        self.index.contains_key(key)
    }

    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.index.keys()
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn compact(&mut self) -> Result<(), StorageError> {
        self.writer.flush()?;

        let live: Vec<(String, Value)> = self.index.keys()
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .map(|k| {
                let v = self.get(&k)?;
                Ok((k, v))
            })
            .collect::<Result<_, StorageError>>()?;

        let tmp_path = self.path.with_extension("ndb.tmp");
        let mut new_pos = 0u64;
        {
            let mut tmp = BufWriter::with_capacity(BUF_CAPACITY, File::create(&tmp_path)?);
            for (key, val) in &live {
                new_pos += write_record(&mut tmp, key, val)?;
            }
            tmp.flush()?;
        }

        std::fs::rename(&tmp_path, &self.path)?;

        let reader     = OpenOptions::new().read(true).open(&self.path)?;
        let write_file = OpenOptions::new().append(true).open(&self.path)?;
        self.reader    = reader;
        self.writer    = BufWriter::with_capacity(BUF_CAPACITY, write_file);
        self.write_pos = new_pos;

        self.index.clear();
        self.cache.clear();
        self.build_index()?;
        Ok(())
    }

    fn build_index(&mut self) -> Result<(), StorageError> {
        self.reader.seek(SeekFrom::Start(0))?;
        let file_len = self.write_pos;
        let mut pos: u64 = 0;

        while pos < file_len {
            let record_start = pos;

            let key_len = read_u32(&mut self.reader)? as usize;
            pos += 4;

            let mut key_buf = vec![0u8; key_len];
            self.reader.read_exact(&mut key_buf)?;
            pos += key_len as u64;

            let val_len = read_u32(&mut self.reader)? as usize;
            pos += 4;

            let mut type_byte = [0u8; 1];
            self.reader.read_exact(&mut type_byte)?;

            let remaining = val_len.saturating_sub(1);
            self.reader.seek(SeekFrom::Current(remaining as i64))?;
            pos += val_len as u64;

            let key = String::from_utf8(key_buf).map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid utf8 key")
            })?;

            if type_byte[0] == crate::types::TYPE_TOMBSTONE {
                self.index.remove(&key);
            } else {
                self.index.insert(key, record_start);
            }
        }
        Ok(())
    }

    fn read_value_at(&mut self, record_offset: u64) -> Result<Value, StorageError> {
        self.reader.seek(SeekFrom::Start(record_offset))?;
        let key_len = read_u32(&mut self.reader)? as usize;
        self.reader.seek(SeekFrom::Current(key_len as i64))?;
        let val_len = read_u32(&mut self.reader)? as usize;
        let mut val_buf = vec![0u8; val_len];
        self.reader.read_exact(&mut val_buf)?;
        let mut pos = 0;
        Ok(decode(&val_buf, &mut pos)?)
    }

    fn append_record(&mut self, key: &str, value: &Value) -> Result<u64, StorageError> {
        let offset = self.write_pos;
        self.write_pos += write_record(&mut self.writer, key, value)?;
        Ok(offset)
    }
}

// Returns bytes written.
fn write_record(w: &mut impl Write, key: &str, value: &Value) -> Result<u64, StorageError> {
    let key_bytes = key.as_bytes();
    let mut val_buf = Vec::new();
    encode(value, &mut val_buf);

    w.write_all(&(key_bytes.len() as u32).to_le_bytes())?;
    w.write_all(key_bytes)?;
    w.write_all(&(val_buf.len() as u32).to_le_bytes())?;
    w.write_all(&val_buf)?;

    Ok((4 + key_bytes.len() + 4 + val_buf.len()) as u64)
}

fn read_u32(r: &mut impl Read) -> Result<u32, StorageError> {
    let mut buf = [0u8; 4];
    r.read_exact(&mut buf)?;
    Ok(u32::from_le_bytes(buf))
}
