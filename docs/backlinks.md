# NarratorDB — Backlinks

## Что это

Backlink-индекс позволяет для любой записи узнать, **кто на неё ссылается** — без сканирования коллекций.

```python
# Кто в таверне?
db.locations["tavern"].backlinks()
# → [{"source_collection": "characters", "source_key": "john",  "source_path": "location"},
#    {"source_collection": "characters", "source_key": "vlad",  "source_path": "location"}]
```

---

## Практические кейсы

### Локации

```python
# Персонаж хранит ссылку на локацию
db.characters["john"]["location"] = ndb.Link("locations/tavern")

# Кто в таверне?
db.locations["tavern"].backlinks()

# Переехать — backlinks обновятся автоматически
db.characters["john"]["location"] = ndb.Link("locations/forest")
```

Модель "персонаж хранит локацию" предпочтительнее чем "локация хранит список персонажей":
при переезде меняется одна запись, а не две.

### Предметы

```python
# У кого предмет?
db.items["item12321"].backlinks()
# → [{"source_collection": "characters", "source_key": "john", "source_path": "body/right_leg"}]
```

### Каскадные проверки

```python
# Можно ли удалить расу?
refs = db.races["human"].backlinks()
if refs:
    raise ValueError(f"На расу ссылаются {len(refs)} записей")
```

### Кто использует прототип

```python
# Какие предметы ссылаются на этот тип?
db.item_types["leg_prosthetic"].backlinks()
```

---

## Как работает

При каждом `set()` движок:

1. Читает **старое** значение → извлекает все статические LINKs
2. Записывает новое значение
3. Извлекает статические LINKs из **нового** значения
4. Вычисляет diff: `removed = old − new`, `added = new − old`
5. Пишет `REMOVE`/`ADD`-записи в `.nbl`-файлы целевых коллекций

При открытии `.nbl`-файл воспроизводится в памяти: `target_key → [BacklinkEntry, ...]`.
Поиск — O(1).

---

## Статические vs динамические LINKs

**Индексируются** только ссылки с известным целевым ключом:
```
items/item12321       ← индексируется
item_types/human_arm  ← индексируется
locations/tavern      ← индексируется
```

**Не индексируется** путь с `$self`:
```
races/$self.identity.race_id/body   ← не индексируется
```

Но статические LINKs внутри `local` такого LINK — **индексируются**:
```python
ndb.Link("races/$self.identity.race_id/body", local={
    "right_leg": ndb.Link("items/item12321")  # ← индексируется
})
```

### Массивы

LINKs внутри массивов тоже индексируются. `source_path` содержит индекс элемента:

```python
db.locations["inn"] = {
    "characters": [ndb.Link("characters/john"), ndb.Link("characters/vlad")]
}

db.characters["john"].backlinks()
# → [{"source_collection": "locations", "source_key": "inn", "source_path": "characters/0"}]
```

---

## Формат `.nbl`-файла

Append-only лог:
```
[op: u8][target_key: TEXT][source_col: TEXT][source_key: TEXT][source_path: TEXT]
```

- `op`: `0x01` = ADD, `0x02` = REMOVE
- `TEXT`: `[len: u32 LE][UTF-8 bytes]`

`source_path` — путь внутри исходной записи через `/`. Пустая строка = корень записи.

---

## Batch-режим

В batch-режиме (`with db:`) backlink-операции накапливаются в `pending_bl`
и применяются атомарно при `commit()`.

---

## BacklinkEntry (Rust)

```rust
pub struct BacklinkEntry {
    pub source_collection: String,
    pub source_key:        String,
    pub source_path:       String,
}
```

В Python возвращается как `dict` с теми же ключами.
