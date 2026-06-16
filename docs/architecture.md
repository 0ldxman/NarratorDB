# NarratorDB — Архитектура

## Цель

NarratorDB — документо-ориентированная БД на Rust с Python-биндингами.
Проектировалась для данных с **прототипным наследованием**: персонажи наследуют тело от расы, предметы — характеристики от типа предмета.

---

## Структура файлов на диске

```
mydb/
  characters.ndb   ← данные коллекции (append-only лог)
  characters.nbl   ← индекс обратных ссылок для коллекции
  races.ndb
  races.nbl
  items.ndb
  items.nbl
```

Один каталог = одна БД. Каждая коллекция — отдельный файл.

---

## Модули

```
src/
  types.rs      → Value, Link, константы типов
  codec.rs      → бинарная сериализация / десериализация
  storage.rs    → Collection — append-only файл + in-memory индекс + кеш
  backlinks.rs  → BacklinkIndex (.nbl) + извлечение статических LINKs
  db.rs         → Database — оркестрирует коллекции, backlinks, batch
  resolver.rs   → Resolver — навигация по LINKs и local-оверрайдам
  python.rs     → PyO3-биндинги (Database, Link, Tombstone)
```

---

## Принципы хранилища

### Append-only лог

Записи никогда не перезаписываются — новая версия дописывается в конец файла.
Удаление — это запись `TOMBSTONE`.

Формат записи в `.ndb`:
```
[key_len: u32 LE][key: bytes][val_len: u32 LE][val: bytes]
```

### In-memory индекс

При открытии коллекция читает файл от начала до конца и строит `HashMap<key → file_offset>`.
Поиск: `get(key)` → offset из HashMap → seek → read. При cache-hit чтение с диска не нужно.

### Кеш записей

`Collection` хранит `HashMap<key → Value>` для записей, изменённых в текущей сессии.
`get()` сначала проверяет кеш, затем идёт на диск.

### Компакция

`Collection::compact()` переписывает файл, оставляя только живые записи.
Вызывается вручную — автоматического триггера нет.

---
---

## Backlinks

При каждом `set()` движок:
1. Читает старое значение → извлекает статические LINKs (`extract_static_links`)
2. Записывает новое значение
3. Вычисляет diff между старыми и новыми LINKs
4. Добавляет/удаляет записи в `.nbl`-файлах целевых коллекций

**Статическая ссылка** — путь без `$self`-сегментов, например `items/item12321`.
Динамические пути (`races/$self.identity.race_id/body`) не индексируются,
но статические LINKs внутри их `local`-оверрайдов — индексируются.

Формат записи в `.nbl`:
```
[op: u8][target_key: TEXT][source_col: TEXT][source_key: TEXT][source_path: TEXT]
```
где `op` = `0x01` (ADD) или `0x02` (REMOVE), `TEXT` = `[len: u32 LE][bytes]`.

---

## Batch-режим

`db.begin()` / `db.commit()`:
- Все `set()`-вызовы пишут в `BufWriter` без flush
- Backlink-операции накапливаются в `pending_bl`
- `commit()` флашит все коллекции, затем применяет backlinks

Без batch каждый `set()` немедленно сбрасывается на диск.

---

## Конкурентность (текущее состояние)

- Каждая `Collection` обёрнута в `Arc<Mutex<Collection>>`
- Запись и чтение сериализованы на уровне коллекции
- Async-runtime (Tokio) не реализован — всё синхронное
