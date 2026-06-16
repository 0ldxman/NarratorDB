from . import _narratordb as _ndb


def _split_colon(key):
    parts = key.split(":")
    return parts[0], parts[1:]


class FieldProxy:
    """Lazy proxy for a field path within a record.

    Materializes automatically on comparison, arithmetic, str(), int(), etc.
    Chain __getitem__ to go deeper; assign via __setitem__ to patch the DB.
    """

    def __init__(self, db, col, key, path):
        self.__dict__["_db"]   = db
        self.__dict__["_col"]  = col
        self.__dict__["_key"]  = key
        self.__dict__["_path"] = path

    # ── path extension ────────────────────────────────────────────────────────

    def __getitem__(self, field):
        return FieldProxy(self._db, self._col, self._key, self._path + [str(field)])

    def __setitem__(self, field, value):
        _patch(self._db, self._col, self._key, self._path + [str(field)], value)

    # ── explicit materialization ──────────────────────────────────────────────

    def __call__(self):
        return _materialize(self._db, self._col, self._key, self._path)

    # ── implicit materialization ──────────────────────────────────────────────

    def __repr__(self):          return repr(self())
    def __str__(self):           return str(self())
    def __format__(self, spec):  return format(self(), spec)
    def __bool__(self):          return bool(self())
    def __int__(self):           return int(self())
    def __float__(self):         return float(self())
    def __len__(self):           return len(self())
    def __iter__(self):          return iter(self())
    def __contains__(self, item): return item in self()
    def __hash__(self):          return hash(self())

    def __eq__(self, other):
        v = self()
        return v == (other() if isinstance(other, FieldProxy) else other)

    def __lt__(self, o): return self() <  (o() if isinstance(o, FieldProxy) else o)
    def __le__(self, o): return self() <= (o() if isinstance(o, FieldProxy) else o)
    def __gt__(self, o): return self() >  (o() if isinstance(o, FieldProxy) else o)
    def __ge__(self, o): return self() >= (o() if isinstance(o, FieldProxy) else o)

    def __add__(self, o):  return self() + o
    def __radd__(self, o): return o + self()
    def __sub__(self, o):  return self() - o
    def __rsub__(self, o): return o - self()
    def __mul__(self, o):  return self() * o
    def __rmul__(self, o): return o * self()

    def backlinks(self):
        """Return all records that hold a static Link pointing to this record."""
        return self._db._inner.backlinks(self._col, self._key)

    @property
    def target(self):
        """Follow a static Link and return a FieldProxy to the target record.

        Use this when you explicitly want to patch the linked record itself,
        not add a local override. Raises TypeError if the value is not a Link
        or if the link path contains $self (dynamic links are not supported).
        """
        val = self()
        if not isinstance(val, _ndb.Link):
            raise TypeError(f"target: expected Link, got {type(val).__name__!r}")
        segments = val.path.split("/")
        if len(segments) < 2:
            raise TypeError(f"target: invalid link path {val.path!r}")
        col, key = segments[0], segments[1]
        if "$" in col or "$" in key:
            raise TypeError(
                f"target: cannot statically resolve dynamic link {val.path!r}; "
                "read the key first and address the record directly"
            )
        sub_path = segments[2:]
        return FieldProxy(self._db, col, key, sub_path)

    # forward unknown attributes to the materialised value (e.g. str.upper(), dict.keys())
    def __getattr__(self, name):
        return getattr(self(), name)


class CollectionProxy:
    """Represents one collection. Supports chained and colon-path access."""

    def __init__(self, db, col):
        self.__dict__["_db"]  = db
        self.__dict__["_col"] = col

    def __getitem__(self, key):
        if ":" in key:
            rec_key, path = _split_colon(key)
            return FieldProxy(self._db, self._col, rec_key, path)
        return FieldProxy(self._db, self._col, key, [])

    def __setitem__(self, key, value):
        if ":" in key:
            rec_key, path = _split_colon(key)
            _patch(self._db, self._col, rec_key, path, value)
        else:
            self._db._inner.set(self._col, key, value)

    def __delitem__(self, key):
        self._db._inner.delete(self._col, key)

    def __contains__(self, key):
        try:
            self._db._inner.get(self._col, key)
            return True
        except KeyError:
            return False


class Database:
    """Ergonomic wrapper around narratordb._narratordb.Database."""

    def __init__(self, path):
        self.__dict__["_inner"] = _ndb.open(path)

    # db.characters or db["characters"]
    def __getattr__(self, name):
        return CollectionProxy(self, name)

    def __getitem__(self, name):
        return CollectionProxy(self, name)

    # batch as context manager: with db: ...
    def __enter__(self):
        self._inner.begin()
        return self

    def __exit__(self, exc_type, *_):
        if exc_type is None:
            self._inner.commit()

    def begin(self):   self._inner.begin()
    def commit(self):  self._inner.commit()

    def resolve(self, col, key, path):
        return self._inner.resolve(col, key, path)

    def backlinks(self, col, key):
        return self._inner.backlinks(col, key)


# ── helpers ───────────────────────────────────────────────────────────────────

def _materialize(db, col, key, path):
    val = db._inner.get(col, key)
    for seg in path:
        if isinstance(val, dict):
            val = val[seg]
        elif isinstance(val, _ndb.Link):
            if seg in val.local:
                val = val.local[seg]
            else:
                raise KeyError(seg)
        else:
            raise TypeError(f"cannot navigate into {type(val).__name__!r} at {seg!r}")
    return val


def _apply_patch(node, path, value):
    """Return a new node with value written at path.

    When a Link is encountered the write goes into link.local —
    the prototype (link.path target) is never modified.
    """
    seg  = path[0]
    rest = path[1:]

    if isinstance(node, dict):
        result = dict(node)
        if rest:
            result[seg] = _apply_patch(result.get(seg, {}), rest, value)
        else:
            result[seg] = value
        return result

    if isinstance(node, _ndb.Link):
        local = dict(node.local)
        if rest:
            local[seg] = _apply_patch(local.get(seg, {}), rest, value)
        else:
            local[seg] = value
        return _ndb.Link(node.path, local)

    raise TypeError(f"cannot patch into {type(node).__name__!r} at {seg!r}")


def _patch(db, col, key, path, value):
    try:
        record = db._inner.get(col, key)
    except KeyError:
        record = {}

    if not path:
        db._inner.set(col, key, value)
        return

    db._inner.set(col, key, _apply_patch(record, path, value))


# ── public API ────────────────────────────────────────────────────────────────

def open(path: str) -> Database:
    return Database(path)


Link      = _ndb.Link
TOMBSTONE = _ndb.TOMBSTONE
Tombstone = _ndb.Tombstone
