use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::backlinks::{BacklinkEntry, BacklinkIndex, LinkRef, extract_static_links};
use crate::storage::{Collection, StorageError};
use crate::types::Value;

enum BlOp {
    Add(String, String, BacklinkEntry),    // (target_col, target_key, entry)
    Remove(String, String, BacklinkEntry),
}

pub struct Database {
    dir: PathBuf,
    collections: HashMap<String, Arc<Mutex<Collection>>>,
    backlink_indexes: HashMap<String, BacklinkIndex>,
    in_batch: bool,
    pending_bl: Vec<BlOp>,
}

impl Database {
    pub fn open(dir: impl AsRef<Path>) -> Result<Self, StorageError> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir)?;

        let mut db = Database {
            dir,
            collections: HashMap::new(),
            backlink_indexes: HashMap::new(),
            in_batch: false,
            pending_bl: Vec::new(),
        };

        for entry in std::fs::read_dir(&db.dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("ndb") {
                if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                    let col = Collection::open(&path)?;
                    db.collections.insert(name.to_string(), Arc::new(Mutex::new(col)));
                }
            }
        }

        Ok(db)
    }

    pub fn begin(&mut self) {
        self.in_batch = true;
        self.pending_bl.clear();
        for col in self.collections.values() {
            col.lock().unwrap().begin_batch();
        }
    }

    pub fn commit(&mut self) -> Result<(), StorageError> {
        // Flush all collections.
        for col in self.collections.values() {
            col.lock().unwrap().commit_batch()?;
        }

        // Apply buffered backlink operations.
        let ops = std::mem::take(&mut self.pending_bl);
        for op in ops {
            match op {
                BlOp::Add(col, key, entry) => {
                    self.backlink_index(&col)?.add(&key, entry)?;
                }
                BlOp::Remove(col, key, entry) => {
                    self.backlink_index(&col)?.remove(&key, &entry)?;
                }
            }
        }

        // Flush backlink index files.
        for bl in self.backlink_indexes.values_mut() {
            bl.flush()?;
        }

        self.in_batch = false;
        Ok(())
    }

    pub fn collection(&mut self, name: &str) -> Result<Arc<Mutex<Collection>>, StorageError> {
        if let Some(col) = self.collections.get(name) {
            return Ok(Arc::clone(col));
        }
        let path = self.dir.join(format!("{}.ndb", name));
        let mut col = Collection::open(&path)?;
        if self.in_batch {
            col.begin_batch();
        }
        let arc = Arc::new(Mutex::new(col));
        self.collections.insert(name.to_string(), Arc::clone(&arc));
        Ok(arc)
    }

    pub fn get(&mut self, collection: &str, key: &str) -> Result<Value, StorageError> {
        let col = self.collection(collection)?;
        let mut guard = col.lock().unwrap();
        guard.get(key)
    }

    pub fn set(&mut self, collection: &str, key: &str, value: &Value) -> Result<(), StorageError> {
        let old_links = match self.get(collection, key) {
            Ok(old_val) => extract_static_links(&old_val, &[]),
            Err(_) => vec![],
        };

        let col = self.collection(collection)?;
        col.lock().unwrap().set(key, value)?;

        let new_links = extract_static_links(value, &[]);
        self.update_backlinks(collection, key, old_links, new_links)?;

        Ok(())
    }

    pub fn set_many(&mut self, ops: Vec<(String, String, Value)>) -> Result<(), StorageError> {
        for (col, key, val) in ops {
            self.set(&col, &key, &val)?;
        }
        Ok(())
    }

    pub fn delete(&mut self, collection: &str, key: &str) -> Result<(), StorageError> {
        self.set(collection, key, &Value::Tombstone)
    }

    pub fn get_backlinks(
        &mut self,
        collection: &str,
        key: &str,
    ) -> Result<Vec<BacklinkEntry>, StorageError> {
        let bl = self.backlink_index(collection)?;
        Ok(bl.get(key).to_vec())
    }

    pub fn collections(&self) -> impl Iterator<Item = &String> {
        self.collections.keys()
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn backlink_index(&mut self, collection: &str) -> Result<&mut BacklinkIndex, StorageError> {
        if !self.backlink_indexes.contains_key(collection) {
            let path = self.dir.join(format!("{}.nbl", collection));
            let bl = BacklinkIndex::open(&path)?;
            self.backlink_indexes.insert(collection.to_string(), bl);
        }
        Ok(self.backlink_indexes.get_mut(collection).unwrap())
    }

    fn update_backlinks(
        &mut self,
        source_collection: &str,
        source_key: &str,
        old_links: Vec<LinkRef>,
        new_links: Vec<LinkRef>,
    ) -> Result<(), StorageError> {
        let old_set: std::collections::HashSet<_> = old_links.iter().collect();
        let new_set: std::collections::HashSet<_> = new_links.iter().collect();

        for link in old_links.iter().filter(|l| !new_set.contains(l)) {
            let entry = BacklinkEntry {
                source_collection: source_collection.to_string(),
                source_key: source_key.to_string(),
                source_path: link.source_path.clone(),
            };
            if self.in_batch {
                self.pending_bl.push(BlOp::Remove(
                    link.target_collection.clone(),
                    link.target_key.clone(),
                    entry,
                ));
            } else {
                let bl = self.backlink_index(&link.target_collection)?;
                bl.remove(&link.target_key, &entry)?;
            }
        }

        for link in new_links.iter().filter(|l| !old_set.contains(l)) {
            let entry = BacklinkEntry {
                source_collection: source_collection.to_string(),
                source_key: source_key.to_string(),
                source_path: link.source_path.clone(),
            };
            if self.in_batch {
                self.pending_bl.push(BlOp::Add(
                    link.target_collection.clone(),
                    link.target_key.clone(),
                    entry,
                ));
            } else {
                let bl = self.backlink_index(&link.target_collection)?;
                bl.add(&link.target_key, entry)?;
            }
        }

        Ok(())
    }
}
