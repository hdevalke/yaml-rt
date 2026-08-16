use std::collections::BTreeMap;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use yaml_rt_serde::value::{Entry, Tag, TaggedValue};
use yaml_rt_serde::{Mapping, Number, Value, from_str, from_value, to_string, to_value};

#[test]
fn parses_and_emits_every_value_shape() {
    let input = concat!(
        "null_value: null\n",
        "boolean: true\n",
        "integer: 340282366920938463463374607431768211455\n",
        "float: -.inf\n",
        "string: \"null\"\n",
        "sequence: [1, two]\n",
        "mapping: {false: value}\n",
        "tagged: !Thing {answer: 42}\n",
    );
    let value: Value = from_str(input).unwrap();

    assert!(value["null_value"].is_null());
    assert_eq!(value["boolean"], true);
    assert_eq!(value["integer"].as_u128(), Some(u128::MAX));
    assert_eq!(value["float"].as_f64(), Some(f64::NEG_INFINITY));
    assert_eq!(value["string"], "null");
    assert_eq!(value["sequence"][0], 1);
    assert_eq!(value["mapping"][Value::Bool(false)], "value");

    let Value::Tagged(tagged) = &value["tagged"] else {
        panic!("expected a tagged value");
    };
    assert!(tagged.tag == "Thing");
    assert_eq!(tagged.value["answer"], 42);

    let reparsed: Value = from_str(&to_string(&value).unwrap()).unwrap();
    assert_eq!(reparsed, value);
}

#[test]
fn aliases_expand_into_independent_values() {
    let value: Value = from_str("base: &base [1, 2]\ncopy: *base\n").unwrap();
    assert_eq!(value["base"], value["copy"]);

    let mut value = value;
    value["copy"][0] = Value::from(9);
    assert_eq!(value["base"][0], 1);
    assert_eq!(value["copy"][0], 9);
}

#[test]
fn number_supports_yaml_serde_accessors_and_128_bit_extensions() {
    let signed = Number::from(i128::MIN);
    let unsigned = Number::from(u128::MAX);
    let float = Number::from(f64::NAN);

    assert_eq!(signed.as_i128(), Some(i128::MIN));
    assert_eq!(signed.as_i64(), None);
    assert_eq!(unsigned.as_u128(), Some(u128::MAX));
    assert_eq!(unsigned.as_u64(), None);
    assert!(float.is_f64());
    assert!(float.is_nan());
    assert!(!float.is_finite());
    assert_eq!(Number::from_str("0x10").unwrap().as_u64(), Some(16));
}

#[test]
fn mappings_preserve_order_replace_and_offer_entry_iteration() {
    let mut mapping = Mapping::with_capacity(3);
    assert_eq!(mapping.insert("first".into(), 1.into()), None);
    assert_eq!(mapping.insert("second".into(), 2.into()), None);
    assert_eq!(mapping.insert("first".into(), 3.into()), Some(1.into()));

    match mapping.entry("third".into()) {
        Entry::Vacant(entry) => {
            entry.insert(4.into());
        }
        Entry::Occupied(_) => panic!("third must be vacant"),
    }
    mapping
        .entry("second".into())
        .and_modify(|value| *value = 5.into())
        .or_insert(Value::Null);

    assert_eq!(
        mapping
            .iter()
            .map(|(key, value)| (key.as_str().unwrap(), value.as_u64().unwrap()))
            .collect::<Vec<_>>(),
        [("first", 3), ("second", 5), ("third", 4)]
    );
    assert_eq!(mapping.shift_remove("second"), Some(5.into()));
    assert_eq!(mapping.keys().collect::<Vec<_>>(), [&"first", &"third"]);
}

#[test]
fn indexing_matches_yaml_serde_read_and_write_behavior() {
    let mut value = Value::Null;
    value["service"]["name"] = Value::from("api");
    assert_eq!(value["service"]["name"], "api");
    assert!(value["missing"][0]["anything"].is_null());

    let mut sequence = Value::from(vec![1, 2]);
    sequence[1] = Value::from(3);
    assert_eq!(sequence.get(1), Some(&Value::from(3)));
    assert_eq!(sequence.get("not-a-sequence-index"), None);
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Config {
    name: String,
    count: u128,
    labels: BTreeMap<String, bool>,
}

#[test]
fn typed_values_convert_without_yaml_text() {
    let config = Config {
        name: "api".to_owned(),
        count: u128::MAX,
        labels: BTreeMap::from([("stable".to_owned(), true)]),
    };

    let value = to_value(&config).unwrap();
    assert_eq!(value["name"], "api");
    assert_eq!(value["count"].as_u128(), Some(u128::MAX));
    assert_eq!(from_value::<Config>(value).unwrap(), config);

    let direct = config.serialize(yaml_rt_serde::value::Serializer).unwrap();
    assert_eq!(direct["labels"]["stable"], true);
}

#[test]
fn tagged_values_and_serde_enums_share_the_same_model() {
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    enum Mode {
        Selected { name: String },
    }

    let mode = Mode::Selected {
        name: "api".to_owned(),
    };
    let value = to_value(&mode).unwrap();
    let Value::Tagged(tagged) = &value else {
        panic!("enum payload must be tagged");
    };
    assert!(tagged.tag == "!Selected");
    assert_eq!(from_value::<Mode>(value.clone()).unwrap(), mode);
    assert_eq!(
        from_str::<Value>(&to_string(&value).unwrap()).unwrap(),
        value
    );

    let manual = Value::Tagged(Box::new(TaggedValue {
        tag: Tag::new("Manual"),
        value: Value::from("payload"),
    }));
    assert_eq!(to_string(&manual).unwrap(), "!Manual payload\n");
}

#[test]
fn apply_merge_handles_aliases_sequences_precedence_and_recursion() {
    let yaml = concat!(
        "first: &first {a: 1, shared: first}\n",
        "second: &second {b: 2, shared: second}\n",
        "target:\n",
        "  <<: [*first, *second]\n",
        "  shared: explicit\n",
        "  nested: {<<: *second, b: 9}\n",
    );
    let mut value: Value = from_str(yaml).unwrap();
    value.apply_merge().unwrap();

    assert_eq!(value["target"]["a"], 1);
    assert_eq!(value["target"]["b"], 2);
    assert_eq!(value["target"]["shared"], "explicit");
    assert_eq!(value["target"]["nested"]["b"], 9);
    assert_eq!(value["target"]["nested"]["shared"], "second");
    assert_eq!(value["target"].get("<<"), None);

    let mut invalid: Value = from_str("target: {<<: nope}\n").unwrap();
    assert!(invalid.apply_merge().is_err());
}

#[test]
fn duplicate_mapping_keys_are_rejected_for_value() {
    let error = from_str::<Value>("key: one\nkey: two\n").unwrap_err();
    assert!(error.to_string().contains("duplicate mapping key"));
}
