use std::collections::HashMap;

use crate::db::Database;
use crate::storage::StorageError;
use crate::types::{Value, Link};

const MAX_DEPTH: usize = 32;

#[derive(Debug)]
pub enum ResolveError {
    Storage(StorageError),
    InvalidPath(String),
    CyclicLink,
    FieldNotFound,
}

impl From<StorageError> for ResolveError {
    fn from(e: StorageError) -> Self { ResolveError::Storage(e) }
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::Storage(e) => write!(f, "storage: {}", e),
            ResolveError::InvalidPath(p) => write!(f, "invalid path: {}", p),
            ResolveError::CyclicLink => write!(f, "cyclic link detected"),
            ResolveError::FieldNotFound => write!(f, "field not found"),
        }
    }
}

// Context of the root record — needed to evaluate $self references.
#[derive(Clone)]
pub struct RootCtx {
    pub collection: String,
    pub key: String,
}

pub struct Resolver<'a> {
    db: &'a mut Database,
}

impl<'a> Resolver<'a> {
    pub fn new(db: &'a mut Database) -> Self {
        Resolver { db }
    }

    // Get a field from a record navigating through nested MAPs and LINKs.
    // field_path is a list of field names to traverse, e.g. ["body", "right_arm"].
    pub fn get_field(
        &mut self,
        collection: &str,
        key: &str,
        field_path: &[&str],
    ) -> Result<Value, ResolveError> {
        let root = RootCtx {
            collection: collection.to_string(),
            key: key.to_string(),
        };
        let record = self.db.get(collection, key)?;
        self.navigate(record, field_path, &root, 0)
    }

    // Fully resolve a value (follow LINK if needed).
    pub fn resolve_value(
        &mut self,
        value: Value,
        root: &RootCtx,
    ) -> Result<Value, ResolveError> {
        self.resolve_inner(value, root, 0)
    }

    fn resolve_inner(
        &mut self,
        value: Value,
        root: &RootCtx,
        depth: usize,
    ) -> Result<Value, ResolveError> {
        if depth > MAX_DEPTH {
            return Err(ResolveError::CyclicLink);
        }
        match value {
            Value::Link(link) => self.follow_link(&link, &[], root, depth + 1),
            other => Ok(other),
        }
    }

    // Navigate field_path starting from `current` value.
    fn navigate(
        &mut self,
        current: Value,
        path: &[&str],
        root: &RootCtx,
        depth: usize,
    ) -> Result<Value, ResolveError> {
        if depth > MAX_DEPTH {
            return Err(ResolveError::CyclicLink);
        }

        if path.is_empty() {
            return self.resolve_inner(current, root, depth);
        }

        let field = path[0];
        let rest = &path[1..];

        match current {
            Value::Map(map) => {
                let val = map.get(field).cloned().unwrap_or(Value::Null);
                self.navigate(val, rest, root, depth)
            }
            Value::Link(link) => {
                // Check local overrides first.
                if let Some(local_val) = link.local.get(field).cloned() {
                    return self.navigate(local_val, rest, root, depth);
                }
                // Not in local — follow the proto path.
                self.follow_link(&link, path, root, depth + 1)
            }
            Value::Null => Ok(Value::Null),
            Value::Tombstone => Ok(Value::Tombstone),
            _ => Err(ResolveError::FieldNotFound),
        }
    }

    // Follow a LINK's path and then navigate `remaining_path` within the target.
    fn follow_link(
        &mut self,
        link: &Link,
        remaining_path: &[&str],
        root: &RootCtx,
        depth: usize,
    ) -> Result<Value, ResolveError> {
        if depth > MAX_DEPTH {
            return Err(ResolveError::CyclicLink);
        }

        let segments: Vec<&str> = link.path.split('/').collect();

        if segments.is_empty() {
            return Err(ResolveError::InvalidPath(link.path.clone()));
        }

        // segments[0] = target collection
        // segments[1] = target key (possibly $self.field.subfield)
        // segments[2..] = sub-path into target record
        let target_col = segments[0];

        let target_key = if segments.len() > 1 {
            self.eval_key_segment(segments[1], root)?
        } else {
            return Err(ResolveError::InvalidPath(link.path.clone()));
        };

        let mut full_path: Vec<&str> = segments[2..].to_vec();
        full_path.extend_from_slice(remaining_path);

        // Fetch target record
        let target_record = match self.db.get(target_col, &target_key) {
            Ok(v) => v,
            Err(StorageError::KeyNotFound) => return Ok(Value::Null),
            Err(e) => return Err(ResolveError::Storage(e)),
        };

        // New root context for $self inside the target (stays as original root)
        self.navigate(target_record, &full_path, root, depth)
    }

    // Evaluate a path segment which may be:
    //   - a literal string: "human", "item12321"
    //   - a self-reference: "$self.identity.race_id" → read from root record
    fn eval_key_segment(&mut self, segment: &str, root: &RootCtx) -> Result<String, ResolveError> {
        if let Some(self_path) = segment.strip_prefix("$self.") {
            let parts: Vec<&str> = self_path.split('.').collect();
            let record = self.db.get(&root.collection, &root.key)?;
            let val = self.navigate(record, &parts, root, 0)?;
            match val {
                Value::Text(s) => Ok(s),
                other => Err(ResolveError::InvalidPath(format!(
                    "$self.{} resolved to {:?}, expected TEXT",
                    self_path, other
                ))),
            }
        } else if segment == "$self" {
            Ok(root.key.clone())
        } else {
            Ok(segment.to_string())
        }
    }
}

// Convenience: extract a nested field from a Map value without a database.
pub fn get_in_map<'a>(map: &'a HashMap<String, Value>, path: &[&str]) -> Option<&'a Value> {
    if path.is_empty() {
        return None;
    }
    let val = map.get(path[0])?;
    if path.len() == 1 {
        Some(val)
    } else {
        match val {
            Value::Map(inner) => get_in_map(inner, &path[1..]),
            _ => None,
        }
    }
}
