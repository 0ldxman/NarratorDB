# NarratorDB — Резолвер

Резолвер (`src/resolver.rs`) — компонент, который умеет **навигировать по вложенным полям**
через MAP-структуры и LINK-ссылки, включая прототипное наследование и `$self`-выражения.

---

## Пример данных

```
collections:
  characters/john → {
      identity: { first_name: "John", race_id: "human" },
      body: Link("races/$self.identity.race_id/body", local={
          left_arm:  Tombstone,
          right_leg: Link("items/item12321")
      }),
      skills: { strength: { exp: 120 } }
  }

  races/human → {
      body: {
          left_arm:  Link("item_types/human_arm"),
          right_arm: Link("item_types/human_arm"),
          right_leg: Link("item_types/human_leg")
      }
  }

  items/item12321 → {
      durability: { current: 100 },
      _type: Link("item_types/leg_prosthetic")
  }

  item_types/human_arm → { capacities: { manipulation: 50 } }
```

---

## Алгоритм `get_field`

```
Resolver::get_field(collection, key, field_path)
```

Шаги:
1. Загружаем корневую запись: `db.get(collection, key)`
2. Рекурсивно навигируем по `field_path` через `navigate()`

### `navigate(current_value, path)`

- `path` пустой → резолвим текущее значение (если LINK — следуем за ним)
- `current` — `Map` → берём `map[path[0]]`, рекурсируем с `path[1..]`
- `current` — `Link` →
  - Есть ли `path[0]` в `link.local`? Если да — берём оттуда
  - Если нет — `follow_link(link, path)`
- `current` — `Null` → возвращаем `Null`
- `current` — `Tombstone` → возвращаем `Tombstone`
- Иначе → `FieldNotFound`

### `follow_link(link, remaining_path)`

1. Разбиваем `link.path` по `/`: `[collection, key_segment, subpath...]`
2. Вычисляем `key_segment`: если начинается с `$self.` — читаем поле корневой записи
3. Загружаем `db.get(collection, resolved_key)`
4. `navigate(target, subpath + remaining_path)`

---

## Разбор конкретного запроса

**`get_field("characters", "john", ["body", "right_arm"])`**

```
1. Загружаем john
2. navigate(john, ["body", "right_arm"])
3. john — Map → берём john["body"]
   = Link("races/$self.identity.race_id/body", local={left_arm: Tombstone, right_leg: ...})
4. navigate(Link(...), ["right_arm"])
5. Link, проверяем local["right_arm"] → нет
6. follow_link(link, ["right_arm"])
7. Разбираем путь: col="races", key="$self.identity.race_id", sub=["body"]
8. $self.identity.race_id → читаем john.identity.race_id = "human"
9. db.get("races", "human") → { body: { left_arm: ..., right_arm: Link("item_types/human_arm") } }
10. navigate(races/human, ["body", "right_arm"])
11. races/human — Map → берём ["body"]
12. { left_arm: ..., right_arm: Link("item_types/human_arm") } — Map → берём ["right_arm"]
13. = Link("item_types/human_arm")
14. path пустой → resolve: follow_link → db.get("item_types", "human_arm")
15. → { capacities: { manipulation: 50 } }
```

**`get_field("characters", "john", ["body", "left_arm"])`**

```
... аналогично до шага 5
5. Link, проверяем local["left_arm"] → есть! = Tombstone
6. navigate(Tombstone, []) → возвращаем Tombstone
```

---

## Защита от циклов

Резолвер отслеживает глубину рекурсии (`depth`).
При `depth > MAX_DEPTH` (32) возвращает `ResolveError::CyclicLink`.

---

## Важно: корневой контекст

`$self` всегда ссылается на **исходную** запись, с которой начался вызов `get_field`.
При переходе в другую коллекцию `$self` не меняется.
