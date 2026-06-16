use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use pyo3::exceptions::{PyKeyError, PyRuntimeError, PyTypeError};
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyBytes, PyDict, PyFloat, PyInt, PyList, PyString};

use crate::db::Database;
use crate::resolver::Resolver;
use crate::types::{Link, Value};

// ── Tombstone sentinel ────────────────────────────────────────────────────────

#[pyclass(name = "Tombstone")]
pub struct PyTombstone;

#[pymethods]
impl PyTombstone {
    fn __repr__(&self) -> &str { "TOMBSTONE" }
    fn __bool__(&self) -> bool { false }
}

// ── Link ─────────────────────────────────────────────────────────────────────

#[pyclass(name = "Link")]
pub struct PyLink {
    pub inner: Link,
}

#[pymethods]
impl PyLink {
    #[new]
    #[pyo3(signature = (path, local=None))]
    fn new(py: Python, path: String, local: Option<Bound<'_, PyDict>>) -> PyResult<Self> {
        let local_map = match local {
            Some(ref d) => py_dict_to_map(py, d)?,
            None => HashMap::new(),
        };
        Ok(PyLink { inner: Link { path, local: local_map } })
    }

    #[getter]
    fn path(&self) -> &str { &self.inner.path }

    #[getter]
    fn local(&self, py: Python) -> PyResult<PyObject> {
        map_to_py(py, &self.inner.local)
    }

    fn __repr__(&self) -> String {
        if self.inner.local.is_empty() {
            format!("Link({:?})", self.inner.path)
        } else {
            format!("Link({:?}, local={{...}})", self.inner.path)
        }
    }
}

// ── Database ─────────────────────────────────────────────────────────────────

#[pyclass(name = "Database")]
pub struct PyDatabase {
    inner: Arc<Mutex<Database>>,
}

#[pymethods]
impl PyDatabase {
    fn get(&self, py: Python, collection: &str, key: &str) -> PyResult<PyObject> {
        let val = self.inner.lock().unwrap()
            .get(collection, key)
            .map_err(|e| PyKeyError::new_err(e.to_string()))?;
        value_to_py(py, val)
    }

    fn set(&self, py: Python, collection: &str, key: &str, value: Bound<'_, PyAny>) -> PyResult<()> {
        let val = py_to_value(py, &value)?;
        self.inner.lock().unwrap()
            .set(collection, key, &val)
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))
    }

    fn set_many(&self, py: Python, ops: Bound<'_, PyList>) -> PyResult<()> {
        use pyo3::types::PyTuple;
        // Lock once and process each record inline — avoids 10k mutex acquisitions
        // and doesn't build an intermediate Vec<Value> (avoids heap pressure).
        let mut db = self.inner.lock().unwrap();
        for item in ops.iter() {
            let t = item.downcast::<PyTuple>()
                .map_err(|_| PyTypeError::new_err("set_many: expected list of (col, key, value) tuples"))?;
            let col: String = t.get_item(0)?.extract()?;
            let key: String = t.get_item(1)?.extract()?;
            let val = py_to_value(py, &t.get_item(2)?)?;
            db.set(&col, &key, &val)
                .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        }
        Ok(())
    }

    fn delete(&self, collection: &str, key: &str) -> PyResult<()> {
        self.inner.lock().unwrap()
            .delete(collection, key)
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))
    }

    fn resolve(&self, py: Python, collection: &str, key: &str, path: Vec<String>) -> PyResult<PyObject> {
        let path_refs: Vec<&str> = path.iter().map(String::as_str).collect();
        let mut db_guard = self.inner.lock().unwrap();
        let val = Resolver::new(&mut *db_guard)
            .get_field(collection, key, &path_refs)
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        value_to_py(py, val)
    }

    fn begin(&self) {
        self.inner.lock().unwrap().begin();
    }

    fn commit(&self) -> PyResult<()> {
        self.inner.lock().unwrap()
            .commit()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))
    }

    fn backlinks(&self, py: Python, collection: &str, key: &str) -> PyResult<PyObject> {
        let entries = self.inner.lock().unwrap()
            .get_backlinks(collection, key)
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;

        let list = PyList::empty_bound(py);
        for entry in entries {
            let d = PyDict::new_bound(py);
            d.set_item("source_collection", &entry.source_collection)?;
            d.set_item("source_key", &entry.source_key)?;
            d.set_item("source_path", &entry.source_path)?;
            list.append(d)?;
        }
        Ok(list.into())
    }

    fn __repr__(&self) -> String {
        let db = self.inner.lock().unwrap();
        format!("Database({:?})", db.dir())
    }
}

// ── Module entry point ────────────────────────────────────────────────────────

#[pyfunction]
fn open(path: &str) -> PyResult<PyDatabase> {
    let db = Database::open(path)
        .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
    Ok(PyDatabase { inner: Arc::new(Mutex::new(db)) })
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyTombstone>()?;
    m.add_class::<PyLink>()?;
    m.add_class::<PyDatabase>()?;
    m.add_function(wrap_pyfunction!(open, m)?)?;
    m.add("TOMBSTONE", PyTombstone {})?;
    Ok(())
}

// ── Type conversions ──────────────────────────────────────────────────────────

pub fn py_to_value(py: Python, obj: &Bound<'_, PyAny>) -> PyResult<Value> {
    if obj.is_none() {
        return Ok(Value::Null);
    }
    if obj.is_instance_of::<PyTombstone>() {
        return Ok(Value::Tombstone);
    }
    if obj.is_instance_of::<PyLink>() {
        let link: PyRef<PyLink> = obj.extract()?;
        return Ok(Value::Link(link.inner.clone()));
    }
    // bool before int — bool subclasses int in Python
    if obj.is_instance_of::<PyBool>() {
        return Ok(Value::Bool(obj.extract::<bool>()?));
    }
    if obj.is_instance_of::<PyInt>() {
        return Ok(Value::Int(obj.extract::<i64>()?));
    }
    if obj.is_instance_of::<PyFloat>() {
        return Ok(Value::Float(obj.extract::<f64>()?));
    }
    if obj.is_instance_of::<PyString>() {
        return Ok(Value::Text(obj.extract::<String>()?));
    }
    if obj.is_instance_of::<PyBytes>() {
        return Ok(Value::Blob(obj.extract::<Vec<u8>>()?));
    }
    if obj.is_instance_of::<PyList>() {
        let list = obj.downcast::<PyList>()?;
        let mut arr = Vec::with_capacity(list.len());
        for item in list.iter() {
            arr.push(py_to_value(py, &item)?);
        }
        return Ok(Value::Array(arr));
    }
    if obj.is_instance_of::<PyDict>() {
        let dict = obj.downcast::<PyDict>()?;
        return Ok(Value::Map(py_dict_to_map(py, dict)?));
    }
    Err(PyTypeError::new_err(format!(
        "cannot convert {} to a NarratorDB value",
        obj.get_type().name()?
    )))
}

fn py_dict_to_map(py: Python, dict: &Bound<'_, PyDict>) -> PyResult<HashMap<String, Value>> {
    let mut map = HashMap::with_capacity(dict.len());
    for (k, v) in dict.iter() {
        let key: String = k.extract()?;
        map.insert(key, py_to_value(py, &v)?);
    }
    Ok(map)
}

pub fn value_to_py(py: Python, value: Value) -> PyResult<PyObject> {
    match value {
        Value::Null         => Ok(py.None()),
        Value::Tombstone    => Ok(Py::new(py, PyTombstone {})?.into_py(py)),
        Value::Bool(b)      => Ok(b.into_py(py)),
        Value::Int(i)       => Ok(i.into_py(py)),
        Value::Float(f)     => Ok(f.into_py(py)),
        Value::Text(s)      => Ok(s.into_py(py)),
        Value::Blob(b)      => Ok(PyBytes::new_bound(py, &b).into()),
        Value::Date(d)      => Ok(d.into_py(py)),
        Value::Time(t)      => Ok(t.into_py(py)),
        Value::DateTime(dt) => Ok(dt.into_py(py)),
        Value::Array(arr)   => {
            let list = PyList::empty_bound(py);
            for item in arr {
                list.append(value_to_py(py, item)?)?;
            }
            Ok(list.into())
        }
        Value::Map(map)     => map_to_py(py, &map),
        Value::Link(link)   => Ok(Py::new(py, PyLink { inner: link })?.into_py(py)),
    }
}

fn map_to_py(py: Python, map: &HashMap<String, Value>) -> PyResult<PyObject> {
    let dict = PyDict::new_bound(py);
    for (k, v) in map {
        dict.set_item(k, value_to_py(py, v.clone())?)?;
    }
    Ok(dict.into())
}
