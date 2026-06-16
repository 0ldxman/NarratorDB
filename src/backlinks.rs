use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::types::Value;

// .nbl record layout:
//   [op: u8][target_key: TEXT][source_col: TEXT][source_key: TEXT][source_path: TEXT]
// TEXT = [len: u32 LE][bytes]
const OP_ADD: u8    = 0x01;
const OP_REMOVE: u8 = 0x02;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BacklinkEntry {
    pub source_collection: String,
    pub source_key: String,
    // Path within the source record where the LINK appears, e.g. "body/right_leg"
    pub source_path: String,
}

pub struct BacklinkIndex {
    file: File,
    // target_key → list of source entries pointing at it
    index: HashMap<String, Vec<BacklinkEntry>>,
    path: PathBuf,
}

impl BacklinkIndex {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, std::io::Error> {
        let path = path.as_ref().to_path_buf();
        let file = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .open(&path)?;

        let mut bl = BacklinkIndex { file, index: HashMap::new(), path };
        bl.build_index()?;
        Ok(bl)
    }

    pub fn get(&self, target_key: &str) -> &[BacklinkEntry] {
        self.index.get(target_key).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn add(&mut self, target_key: &str, entry: BacklinkEntry) -> Result<(), std::io::Error> {
        self.write_record(OP_ADD, target_key, &entry)?;
        self.index.entry(target_key.to_string()).or_default().push(entry);
        Ok(())
    }

    pub fn remove(&mut self, target_key: &str, entry: &BacklinkEntry) -> Result<(), std::io::Error> {
        self.write_record(OP_REMOVE, target_key, entry)?;
        if let Some(entries) = self.index.get_mut(target_key) {
            entries.retain(|e| e != entry);
        }
        Ok(())
    }

    pub fn flush(&mut self) -> Result<(), std::io::Error> {
        self.file.flush()
    }

    pub fn compact(&mut self) -> Result<(), std::io::Error> {
        let tmp_path = self.path.with_extension("nbl.tmp");
        {
            let mut tmp = File::create(&tmp_path)?;
            for (target_key, entries) in &self.index {
                for entry in entries {
                    write_record_to(&mut tmp, OP_ADD, target_key, entry)?;
                }
            }
            tmp.flush()?;
        }
        std::fs::rename(&tmp_path, &self.path)?;
        self.file = OpenOptions::new().read(true).append(true).open(&self.path)?;
        Ok(())
    }

    fn build_index(&mut self) -> Result<(), std::io::Error> {
        self.file.seek(SeekFrom::Start(0))?;
        loop {
            let op = match read_u8(&mut self.file) {
                Ok(b) => b,
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e),
            };
            let target_key  = read_string(&mut self.file)?;
            let source_col  = read_string(&mut self.file)?;
            let source_key  = read_string(&mut self.file)?;
            let source_path = read_string(&mut self.file)?;

            let entry = BacklinkEntry {
                source_collection: source_col,
                source_key,
                source_path,
            };
            match op {
                OP_ADD    => { self.index.entry(target_key).or_default().push(entry); }
                OP_REMOVE => {
                    if let Some(v) = self.index.get_mut(&target_key) {
                        v.retain(|e| e != &entry);
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn write_record(&mut self, op: u8, target_key: &str, entry: &BacklinkEntry) -> Result<(), std::io::Error> {
        write_record_to(&mut self.file, op, target_key, entry)?;
        self.file.flush()
    }
}

fn write_record_to(file: &mut File, op: u8, target_key: &str, entry: &BacklinkEntry) -> Result<(), std::io::Error> {
    file.write_all(&[op])?;
    write_string(file, target_key)?;
    write_string(file, &entry.source_collection)?;
    write_string(file, &entry.source_key)?;
    write_string(file, &entry.source_path)?;
    Ok(())
}

fn write_string(file: &mut File, s: &str) -> Result<(), std::io::Error> {
    let b = s.as_bytes();
    file.write_all(&(b.len() as u32).to_le_bytes())?;
    file.write_all(b)
}

fn read_u8(file: &mut File) -> Result<u8, std::io::Error> {
    let mut buf = [0u8; 1];
    file.read_exact(&mut buf)?;
    Ok(buf[0])
}

fn read_string(file: &mut File) -> Result<String, std::io::Error> {
    let mut len_buf = [0u8; 4];
    file.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    file.read_exact(&mut buf)?;
    String::from_utf8(buf)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid utf8 in backlink"))
}

// --- Link extraction ---

// Extracted reference to another record found somewhere in a value tree.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct LinkRef {
    pub target_collection: String,
    pub target_key: String,
    pub source_path: String,  // path within the source record, slash-separated
}

// Walk a Value tree and collect all static LINKs (no $self segments).
// `path_prefix` is the path already traversed to reach this value.
pub fn extract_static_links(value: &Value, path_prefix: &[String]) -> Vec<LinkRef> {
    let mut out = Vec::new();
    collect_links(value, path_prefix, &mut out);
    out
}

fn collect_links(value: &Value, current_path: &[String], out: &mut Vec<LinkRef>) {
    match value {
        Value::Map(m) => {
            for (k, v) in m {
                let mut path = current_path.to_vec();
                path.push(k.clone());
                collect_links(v, &path, out);
            }
        }
        Value::Link(link) => {
            if let Some((col, key)) = parse_static_target(&link.path) {
                out.push(LinkRef {
                    target_collection: col,
                    target_key: key,
                    source_path: current_path.join("/"),
                });
            }
            // Recurse into local overrides — they may contain static LINKs.
            collect_links_in_local(&link.local, current_path, out);
        }
        Value::Array(arr) => {
            for (i, v) in arr.iter().enumerate() {
                let mut path = current_path.to_vec();
                path.push(i.to_string());
                collect_links(v, &path, out);
            }
        }
        _ => {}
    }
}

fn collect_links_in_local(local: &HashMap<String, Value>, current_path: &[String], out: &mut Vec<LinkRef>) {
    for (k, v) in local {
        let mut path = current_path.to_vec();
        path.push(k.clone());
        collect_links(v, &path, out);
    }
}

// Returns (collection, key) if the LINK path has no dynamic segments.
// Path format: "collection/key/..." where key must not start with '$'.
fn parse_static_target(path: &str) -> Option<(String, String)> {
    let mut segments = path.splitn(3, '/');
    let col = segments.next()?;
    let key = segments.next()?;
    if col.is_empty() || key.starts_with('$') {
        return None;
    }
    Some((col.to_string(), key.to_string()))
}
