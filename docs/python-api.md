# NarratorDB — Python API

Два уровня API:

| Модуль | Описание |
|--------|----------|
| `narratordb` | Низкоуровневый Rust-биндинг (`narratordb.so`) |
| `ndb` | Ergonomic-обёртка с синтаксическим сахаром (рекомендуется) |

---

## Быстрый старт

```python
import ndb

db = ndb.open("/path/to/mydb")

# Записать запись целиком
db.characters["john"] = {
    "identity": {"first_name": "John", "race_id": "human"},
    "location": ndb.Link("locations/tavern"),
    "body": ndb.Link("races/$self.identity.race_id/body", local={
        "left_arm":  ndb.TOMBSTONE,
        "right_leg": ndb.Link("items/item12321"),
    }),
}

# Читать поля
db.characters["john"]["identity"]["first_name"]   # → "John"
db.characters["john:identity:first_name"]          # то же самое

# Писать поля
db.characters["john"]["identity"]["first_name"] = "James"
db.characters["john:identity:first_name"] = "James"

# Удалить запись
del db.characters["john"]

# Проверить наличие
"john" in db.characters
```

---

## `ndb.open(path) → Database`

Открывает (или создаёт) БД в указанном каталоге.

```python
db = ndb.open("/path/to/mydb")
```

---

## `Database`

### Доступ к коллекциям

```python
db.characters        # через атрибут
db["characters"]     # через ключ — одно и то же
```

Возвращает `CollectionProxy`.

### Batch через контекст-менеджер

```python
with db:
    db.characters["john"] = {...}
    db.characters["vlad"] = {...}
# commit() вызывается автоматически при выходе без исключения
```

### `db.begin()` / `db.commit()`

Явное управление batch-режимом. `rollback()` не реализован — просто не вызывать `commit()`.

### `db.resolve(col, key, path: list[str])`

Навигация через LINK-цепочки с поддержкой прототипного наследования и `$self`.

```python
db.resolve("characters", "john", ["body", "right_arm"])
# → {"capacities": {"manipulation": 50}}  (fallback на прототип расы)

db.resolve("characters", "john", ["body", "left_arm"])
# → TOMBSTONE
```

### `db.backlinks(col, key)`

Низкоуровневый вызов. Предпочтительнее использовать `.backlinks()` на прокси (см. ниже).

---

## `CollectionProxy`

Получается через `db.characters` или `db["characters"]`.

### Чтение и запись записей

```python
db.characters["john"]           # → FieldProxy (ленивый)
db.characters["john"] = {...}   # записать всю запись
del db.characters["john"]       # удалить
"john" in db.characters         # проверить наличие
```

### Colon-path — плоский доступ к вложенным полям

```python
db.characters["john:identity:first_name"]          # читать
db.characters["john:identity:first_name"] = "James" # писать
```

Первый сегмент до `:` — ключ записи, остальные — путь вглубь.

---

## `FieldProxy`

Ленивый прокси на поле (или запись целиком). Материализуется автоматически при использовании.

### Чейнинг — доступ на любую глубину

```python
db.characters["john"]["identity"]["first_name"]              # читать
db.characters["john"]["skills"]["strength"]["exp"] = 200     # писать
```

### Материализация

Прокси ведёт себя как значение в большинстве контекстов:

```python
name = db.characters["john"]["identity"]["first_name"]

print(name)             # → "John"
name == "John"          # → True
name.upper()            # → "JOHN"
f"Привет, {name}!"     # → "Привет, John!"
int(db.characters["john"]["level"]) + 5
```

Явная материализация нужна редко, но доступна:
```python
name = db.characters["john"]["identity"]["first_name"]()
```

### Запись через LINK — вариант A (по умолчанию)

При записи через LINK-цепочку значение пишется в `local`-оверрайд.
Прототип (целевая запись линка) **не трогается**.

```python
# john.body — Link на расу.  Запись идёт в john.body.local, не в расу
db.characters["john"]["body"]["left_arm"] = ndb.Link("items/new_arm")

# john.body.right_leg — Link на item12321. Пишем в right_leg.local
db.characters["john"]["body"]["right_leg"]["durability"]["current"] = 80
# → john хранит оверрайд; items/item12321 не изменился
```

### `.target` — явный переход в целевую запись (вариант B)

Когда нужно изменить именно ту запись, на которую указывает LINK:

```python
# Патчим сам предмет items/item12321, а не john
db.characters["john"]["body"]["right_leg"].target["durability"]["current"] = 55
```

Работает только со статическими линками (без `$self`).
Для динамических — читай ключ и обращайся к записи напрямую:

```python
item_key = db.characters["john"]["body"]["right_leg"].path.split("/")[1]
db.items[item_key]["durability"]["current"] = 55
```

### `.backlinks()`

Возвращает все записи, ссылающиеся на данную через статический LINK.

```python
# Кто в таверне?
db.locations["tavern"].backlinks()
# → [{"source_collection": "characters", "source_key": "john",  "source_path": "location"},
#    {"source_collection": "characters", "source_key": "vlad",  "source_path": "location"}]

# Кто носит этот предмет?
db.items["item12321"].backlinks()
# → [{"source_collection": "characters", "source_key": "john", "source_path": "body/right_leg"}]
```

Каждый элемент:
- `source_collection` — коллекция ссылающейся записи
- `source_key` — ключ ссылающейся записи
- `source_path` — путь внутри записи, где стоит LINK (через `/`)

Backlinks обновляются автоматически при каждом `set`. Поиск — O(1).

---

## `ndb.Link`

```python
ndb.Link(path: str, local: dict | None = None)
```

| Атрибут  | Тип    | Описание |
|----------|--------|----------|
| `.path`  | `str`  | Путь вида `col/key[/subpath]` |
| `.local` | `dict` | Локальные оверрайды |

```python
# Простая ссылка
ndb.Link("item_types/human_arm")

# Прототипное наследование
ndb.Link("races/$self.identity.race_id/body", local={
    "left_arm":  ndb.TOMBSTONE,
    "right_leg": ndb.Link("items/item12321"),
})
```

---

## `ndb.TOMBSTONE`

Явное отсутствие значения. Останавливает цепочку прототипного наследования.

- `bool(TOMBSTONE)` → `False`
- `repr(TOMBSTONE)` → `"TOMBSTONE"`

```python
db.characters["john"]["body"]["left_arm"] = ndb.TOMBSTONE  # "руки нет"
```

---

## Маппинг типов Python → Value

| Python      | Value       |
|-------------|-------------|
| `None`      | `Null`      |
| `TOMBSTONE` | `Tombstone` |
| `Link`      | `Link`      |
| `bool`      | `Bool`      |
| `int`       | `Int`       |
| `float`     | `Float`     |
| `str`       | `Text`      |
| `bytes`     | `Blob`      |
| `list`      | `Array`     |
| `dict`      | `Map`       |

## Маппинг Value → Python

| Value       | Python        |
|-------------|---------------|
| `Null`      | `None`        |
| `Tombstone` | `TOMBSTONE`   |
| `Link`      | `Link`        |
| `Bool`      | `bool`        |
| `Int`       | `int`         |
| `Float`     | `float`       |
| `Text`      | `str`         |
| `Blob`      | `bytes`       |
| `Date`      | `int` (дней)  |
| `Time`      | `int` (сек)   |
| `DateTime`  | `int` (unix)  |
| `Array`     | `list`        |
| `Map`       | `dict`        |
