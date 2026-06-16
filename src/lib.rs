pub mod types;
pub mod codec;
pub mod storage;
pub mod backlinks;
pub mod db;
pub mod resolver;
pub mod python;

use pyo3::prelude::*;

#[pymodule]
fn _narratordb(m: &Bound<'_, PyModule>) -> PyResult<()> {
    python::register(m)
}

#[cfg(test)]
mod backlink_tests {
    use crate::db::Database;
    use crate::types::{Value, Link};
    use std::collections::HashMap;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("narratordb_bl_{}", name));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn test_backlink_added_on_set() {
        let dir = temp_dir("add");
        let mut db = Database::open(&dir).unwrap();

        let val = Value::Link(Link::simple("items/item12321"));
        db.set("characters", "john", &val).unwrap();

        let bls = db.get_backlinks("items", "item12321").unwrap();
        assert_eq!(bls.len(), 1);
        assert_eq!(bls[0].source_collection, "characters");
        assert_eq!(bls[0].source_key, "john");
        assert_eq!(bls[0].source_path, "");
    }

    #[test]
    fn test_backlink_nested_path() {
        let dir = temp_dir("nested");
        let mut db = Database::open(&dir).unwrap();

        let mut body_local = HashMap::new();
        body_local.insert("right_leg".into(), Value::Link(Link::simple("items/item12321")));
        let mut john = HashMap::new();
        john.insert("body".into(), Value::Link(Link::with_local(
            "races/$self.identity.race_id/body",
            body_local,
        )));
        db.set("characters", "john", &Value::Map(john)).unwrap();

        let bls = db.get_backlinks("items", "item12321").unwrap();
        assert_eq!(bls.len(), 1);
        assert_eq!(bls[0].source_path, "body/right_leg");
    }

    #[test]
    fn test_backlink_removed_on_overwrite() {
        let dir = temp_dir("remove");
        let mut db = Database::open(&dir).unwrap();

        db.set("characters", "john", &Value::Link(Link::simple("items/sword"))).unwrap();
        // Overwrite with different value — no more LINK to items/sword
        db.set("characters", "john", &Value::Int(42)).unwrap();

        let bls = db.get_backlinks("items", "sword").unwrap();
        assert!(bls.is_empty());
    }

    #[test]
    fn test_backlink_updated_on_overwrite() {
        let dir = temp_dir("update");
        let mut db = Database::open(&dir).unwrap();

        db.set("characters", "john", &Value::Link(Link::simple("items/sword"))).unwrap();
        db.set("characters", "john", &Value::Link(Link::simple("items/shield"))).unwrap();

        assert!(db.get_backlinks("items", "sword").unwrap().is_empty());
        assert_eq!(db.get_backlinks("items", "shield").unwrap().len(), 1);
    }

    #[test]
    fn test_multiple_sources() {
        let dir = temp_dir("multi");
        let mut db = Database::open(&dir).unwrap();

        db.set("characters", "john", &Value::Link(Link::simple("items/sword"))).unwrap();
        db.set("characters", "vlad", &Value::Link(Link::simple("items/sword"))).unwrap();

        let bls = db.get_backlinks("items", "sword").unwrap();
        assert_eq!(bls.len(), 2);
        let keys: Vec<&str> = bls.iter().map(|b| b.source_key.as_str()).collect();
        assert!(keys.contains(&"john"));
        assert!(keys.contains(&"vlad"));
    }

    #[test]
    fn test_backlink_persists_across_reopen() {
        let dir = temp_dir("persist");
        {
            let mut db = Database::open(&dir).unwrap();
            db.set("characters", "john", &Value::Link(Link::simple("items/item12321"))).unwrap();
        }
        let mut db = Database::open(&dir).unwrap();
        let bls = db.get_backlinks("items", "item12321").unwrap();
        assert_eq!(bls.len(), 1);
        assert_eq!(bls[0].source_key, "john");
    }

    #[test]
    fn test_dynamic_link_not_indexed() {
        // $self links should NOT be indexed as static backlinks
        let dir = temp_dir("dynamic");
        let mut db = Database::open(&dir).unwrap();

        let mut local = HashMap::new();
        local.insert("leg".into(), Value::Link(Link::simple("items/item12321")));
        let link = Value::Link(Link::with_local("races/$self.identity.race_id/body", local));
        db.set("characters", "john", &link).unwrap();

        // Dynamic path not indexed
        let bls_races = db.get_backlinks("races", "human").unwrap();
        assert!(bls_races.is_empty());

        // But static link inside local IS indexed
        let bls_items = db.get_backlinks("items", "item12321").unwrap();
        assert_eq!(bls_items.len(), 1);
    }
}

#[cfg(test)]
mod resolver_tests {
    use crate::db::Database;
    use crate::resolver::Resolver;
    use crate::types::{Value, Link};
    use std::collections::HashMap;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("narratordb_resolver_{}", name));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn setup_world(db: &mut Database) {
        // --- skill definitions ---
        let mut strength_def = HashMap::new();
        strength_def.insert("name".into(), Value::Text("Сила".into()));
        strength_def.insert("desc".into(), Value::Text("Физическая сила".into()));
        db.set("skills", "strength", &Value::Map(strength_def)).unwrap();

        let mut agility_def = HashMap::new();
        agility_def.insert("name".into(), Value::Text("Ловкость".into()));
        agility_def.insert("desc".into(), Value::Text("Ловкость персонажа".into()));
        db.set("skills", "agility", &Value::Map(agility_def)).unwrap();

        // --- item types ---
        let mut human_arm = HashMap::new();
        let mut arm_caps = HashMap::new();
        arm_caps.insert("manipulation".into(), Value::Int(50));
        human_arm.insert("capacities".into(), Value::Map(arm_caps));
        db.set("item_types", "human_arm", &Value::Map(human_arm)).unwrap();

        let mut leg_prosthetic = HashMap::new();
        let mut leg_caps = HashMap::new();
        leg_caps.insert("walk".into(), Value::Int(120));
        leg_prosthetic.insert("capacities".into(), Value::Map(leg_caps));
        let mut leg_dur = HashMap::new();
        leg_dur.insert("max_durability".into(), Value::Int(120));
        leg_prosthetic.insert("durability".into(), Value::Map(leg_dur));
        db.set("item_types", "leg_prosthetic", &Value::Map(leg_prosthetic)).unwrap();

        // --- item instances ---
        // item12321 is a leg_prosthetic instance with current durability
        let mut item = HashMap::new();
        let mut item_dur = HashMap::new();
        item_dur.insert("current".into(), Value::Int(100));
        item.insert("durability".into(), Value::Map(item_dur));
        // LINK to item_type for static props
        item.insert("_type".into(), Value::Link(Link::simple("item_types/leg_prosthetic")));
        db.set("items", "item12321", &Value::Map(item)).unwrap();

        // --- races ---
        let mut human_body = HashMap::new();
        human_body.insert("left_arm".into(),  Value::Link(Link::simple("item_types/human_arm")));
        human_body.insert("right_arm".into(), Value::Link(Link::simple("item_types/human_arm")));
        human_body.insert("right_leg".into(), Value::Link(Link::simple("item_types/human_leg")));
        let mut human = HashMap::new();
        human.insert("body".into(), Value::Map(human_body));
        db.set("races", "human", &Value::Map(human)).unwrap();

        // --- john ---
        let mut identity = HashMap::new();
        identity.insert("first_name".into(), Value::Text("John".into()));
        identity.insert("last_name".into(),  Value::Text("Snow".into()));
        identity.insert("race_id".into(),    Value::Text("human".into()));
        identity.insert("gender".into(),     Value::Text("male".into()));

        let mut body_local = HashMap::new();
        body_local.insert("left_arm".into(),  Value::Tombstone);
        body_local.insert("right_leg".into(), Value::Link(Link::simple("items/item12321")));

        let mut skills = HashMap::new();
        let mut str_inst = HashMap::new();
        str_inst.insert("exp".into(), Value::Int(120));
        skills.insert("strength".into(), Value::Map(str_inst));
        let mut agi_inst = HashMap::new();
        agi_inst.insert("exp".into(), Value::Int(100));
        skills.insert("agility".into(), Value::Map(agi_inst));

        let mut john = HashMap::new();
        john.insert("identity".into(), Value::Map(identity));
        john.insert("body".into(), Value::Link(Link::with_local(
            "races/$self.identity.race_id/body",
            body_local,
        )));
        john.insert("skills".into(), Value::Map(skills));

        db.set("characters", "john", &Value::Map(john)).unwrap();
    }

    #[test]
    fn test_simple_field() {
        let dir = temp_dir("simple");
        let mut db = Database::open(&dir).unwrap();
        setup_world(&mut db);

        let val = Resolver::new(&mut db)
            .get_field("characters", "john", &["identity", "first_name"])
            .unwrap();
        assert_eq!(val, Value::Text("John".into()));
    }

    #[test]
    fn test_tombstone_no_arm() {
        // john.body.left_arm = TOMBSTONE → руки нет
        let dir = temp_dir("tombstone");
        let mut db = Database::open(&dir).unwrap();
        setup_world(&mut db);

        let val = Resolver::new(&mut db)
            .get_field("characters", "john", &["body", "left_arm"])
            .unwrap();
        assert_eq!(val, Value::Tombstone);
    }

    #[test]
    fn test_link_override_right_leg() {
        // john.body.right_leg = LINK("items/item12321") → local override wins
        let dir = temp_dir("right_leg");
        let mut db = Database::open(&dir).unwrap();
        setup_world(&mut db);

        // Should resolve to the item record (Map with durability etc.)
        let val = Resolver::new(&mut db)
            .get_field("characters", "john", &["body", "right_leg"])
            .unwrap();
        // right_leg resolves to LINK("items/item12321") which is itself a Map
        // get_field returns the resolved value
        match val {
            Value::Map(m) => assert!(m.contains_key("durability")),
            other => panic!("expected Map, got {:?}", other),
        }
    }

    #[test]
    fn test_proto_fallback_right_arm() {
        // john.body.right_arm not in local → falls back to races/human/body/right_arm
        let dir = temp_dir("right_arm");
        let mut db = Database::open(&dir).unwrap();
        setup_world(&mut db);

        let val = Resolver::new(&mut db)
            .get_field("characters", "john", &["body", "right_arm"])
            .unwrap();
        // races/human/body/right_arm = LINK("item_types/human_arm")
        // which resolves to { capacities: { manipulation: 50 } }
        match val {
            Value::Map(m) => assert!(m.contains_key("capacities")),
            other => panic!("expected Map with capacities, got {:?}", other),
        }
    }

    #[test]
    fn test_self_ref_in_path() {
        // Directly test $self resolution: races/$self.identity.race_id
        // resolved from john's context should give races/human
        let dir = temp_dir("self_ref");
        let mut db = Database::open(&dir).unwrap();
        setup_world(&mut db);

        let val = Resolver::new(&mut db)
            .get_field("characters", "john", &["body"])
            .unwrap();
        // body is a LINK, resolving it without further path gives the proto Map
        match val {
            Value::Map(m) => assert!(m.contains_key("right_arm")),
            other => panic!("expected body Map, got {:?}", other),
        }
    }

    #[test]
    fn test_skill_instance_data() {
        // john.skills.strength.exp = 120 (instance data, no LINK needed)
        let dir = temp_dir("skill_exp");
        let mut db = Database::open(&dir).unwrap();
        setup_world(&mut db);

        let val = Resolver::new(&mut db)
            .get_field("characters", "john", &["skills", "strength", "exp"])
            .unwrap();
        assert_eq!(val, Value::Int(120));
    }
}

#[cfg(test)]
mod storage_tests {
    use crate::db::Database;
    use crate::types::Value;
    use std::collections::HashMap;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("narratordb_test_{}", name));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn test_set_get() {
        let dir = temp_dir("set_get");
        let mut db = Database::open(&dir).unwrap();
        db.set("characters", "john", &Value::Text("John Snow".into())).unwrap();
        let val = db.get("characters", "john").unwrap();
        assert_eq!(val, Value::Text("John Snow".into()));
    }

    #[test]
    fn test_delete() {
        let dir = temp_dir("delete");
        let mut db = Database::open(&dir).unwrap();
        db.set("characters", "john", &Value::Int(1)).unwrap();
        db.delete("characters", "john").unwrap();
        assert!(db.get("characters", "john").is_err());
    }

    #[test]
    fn test_overwrite() {
        let dir = temp_dir("overwrite");
        let mut db = Database::open(&dir).unwrap();
        db.set("items", "sword", &Value::Int(1)).unwrap();
        db.set("items", "sword", &Value::Int(2)).unwrap();
        assert_eq!(db.get("items", "sword").unwrap(), Value::Int(2));
    }

    #[test]
    fn test_persistence() {
        let dir = temp_dir("persist");
        {
            let mut db = Database::open(&dir).unwrap();
            db.set("races", "human", &Value::Bool(true)).unwrap();
        }
        // Reopen — index must be rebuilt from file
        let mut db = Database::open(&dir).unwrap();
        assert_eq!(db.get("races", "human").unwrap(), Value::Bool(true));
    }

    #[test]
    fn test_tombstone_persistence() {
        let dir = temp_dir("tombstone");
        {
            let mut db = Database::open(&dir).unwrap();
            db.set("skills", "strength", &Value::Int(100)).unwrap();
            db.delete("skills", "strength").unwrap();
        }
        let mut db = Database::open(&dir).unwrap();
        assert!(db.get("skills", "strength").is_err());
    }

    #[test]
    fn test_map_value() {
        let dir = temp_dir("map_value");
        let mut map = HashMap::new();
        map.insert("first_name".into(), Value::Text("John".into()));
        map.insert("age".into(), Value::Int(30));
        let mut db = Database::open(&dir).unwrap();
        db.set("characters", "john", &Value::Map(map.clone())).unwrap();
        assert_eq!(db.get("characters", "john").unwrap(), Value::Map(map));
    }

    #[test]
    fn test_compact() {
        let dir = temp_dir("compact");
        let mut db = Database::open(&dir).unwrap();
        for i in 0..10 {
            db.set("items", "key", &Value::Int(i)).unwrap();
        }
        let col = db.collection("items").unwrap();
        col.lock().unwrap().compact().unwrap();
        drop(col);
        assert_eq!(db.get("items", "key").unwrap(), Value::Int(9));
    }
}

#[cfg(test)]
mod tests {
    use super::codec::{encode, decode};
    use super::types::*;
    use std::collections::HashMap;

    fn roundtrip(value: &Value) -> Value {
        let mut buf = Vec::new();
        encode(value, &mut buf);
        let mut pos = 0;
        decode(&buf, &mut pos).unwrap()
    }

    #[test]
    fn test_primitives() {
        assert_eq!(roundtrip(&Value::Null), Value::Null);
        assert_eq!(roundtrip(&Value::Tombstone), Value::Tombstone);
        assert_eq!(roundtrip(&Value::Bool(true)), Value::Bool(true));
        assert_eq!(roundtrip(&Value::Bool(false)), Value::Bool(false));
        assert_eq!(roundtrip(&Value::Int(-42)), Value::Int(-42));
        assert_eq!(roundtrip(&Value::Float(3.14)), Value::Float(3.14));
        assert_eq!(roundtrip(&Value::Text("hello".into())), Value::Text("hello".into()));
        assert_eq!(roundtrip(&Value::Blob(vec![1, 2, 3])), Value::Blob(vec![1, 2, 3]));
        assert_eq!(roundtrip(&Value::Date(19900)), Value::Date(19900));
        assert_eq!(roundtrip(&Value::Time(3600)), Value::Time(3600));
        assert_eq!(roundtrip(&Value::DateTime(1718000000)), Value::DateTime(1718000000));
    }

    #[test]
    fn test_array() {
        let arr = Value::Array(vec![
            Value::Int(1),
            Value::Text("two".into()),
            Value::Bool(false),
        ]);
        assert_eq!(roundtrip(&arr), arr);
    }

    #[test]
    fn test_map() {
        let mut map = HashMap::new();
        map.insert("name".into(), Value::Text("John".into()));
        map.insert("age".into(), Value::Int(30));
        let val = Value::Map(map);
        assert_eq!(roundtrip(&val), val);
    }

    #[test]
    fn test_link_simple() {
        let link = Value::Link(Link::simple("items.item12321"));
        assert_eq!(roundtrip(&link), link);
    }

    #[test]
    fn test_link_with_local() {
        let mut local = HashMap::new();
        local.insert("left_arm".into(), Value::Tombstone);
        local.insert("right_leg".into(), Value::Link(Link::simple("items.item12321")));
        let link = Value::Link(Link::with_local(
            "races.$self.identity.race_id.body",
            local,
        ));
        assert_eq!(roundtrip(&link), link);
    }

    #[test]
    fn test_nested_map() {
        let mut inner = HashMap::new();
        inner.insert("exp".into(), Value::Int(120));

        let mut skills = HashMap::new();
        skills.insert("strength".into(), Value::Map(inner));

        let val = Value::Map(skills);
        assert_eq!(roundtrip(&val), val);
    }
}
