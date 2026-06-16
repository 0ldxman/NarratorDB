# NarratorDB — Документация

Document-oriented БД на Rust с Python-биндингами.
Проектировалась для RP-систем с прототипным наследованием: персонаж наследует тело от расы, предмет — характеристики от типа.

## Разделы

- [Архитектура](architecture.md) — хранилище, форматы файлов, batch, конкурентность
- [Типы данных](types.md) — Value, Link, Null vs Tombstone
- [Резолвер](resolver.md) — навигация по LINKs, $self, прототипное наследование
- [Backlinks](backlinks.md) — индекс обратных ссылок, практические кейсы
- [Python API](python-api.md) — полный справочник ndb и narratordb

## Быстрый старт

```python
import ndb

db = ndb.open("/path/to/mydb")

# Записать данные
db.races["human"] = {
    "body": {
        "left_arm":  ndb.Link("item_types/human_arm"),
        "right_arm": ndb.Link("item_types/human_arm"),
    }
}

db.characters["john"] = {
    "identity": {"first_name": "John", "race_id": "human"},
    "location": ndb.Link("locations/tavern"),
    "body": ndb.Link("races/$self.identity.race_id/body", local={
        "left_arm":  ndb.TOMBSTONE,                    # руки нет
        "right_leg": ndb.Link("items/item12321"),       # протез
    }),
}

# Читать поля — чейнинг
db.characters["john"]["identity"]["first_name"]   # → "John"

# Или через colon-path
db.characters["john:identity:first_name"]          # → "John"

# Писать поля
db.characters["john"]["identity"]["first_name"] = "James"
db.characters["john"]["level"] = 10

# Резолв через LINK-цепочку (прототипное наследование)
db.resolve("characters", "john", ["body", "right_arm"])
# → {"capacities": {"manipulation": 50}}  (из расы)

db.resolve("characters", "john", ["body", "left_arm"])
# → TOMBSTONE

# Локации — backlinks
db.locations["tavern"].backlinks()
# → [{"source_collection": "characters", "source_key": "john", "source_path": "location"}]

# Переезд
db.characters["john"]["location"] = ndb.Link("locations/forest")

# Batch-запись
with db:
    for i in range(1000):
        db.characters[f"npc_{i}"] = {"level": 1}
```
