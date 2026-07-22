use std::collections::BTreeMap;
use std::io::Cursor;

use serde::{Deserialize, Serialize};
use yaml_rt_serde::{
    Deserializer, Serializer, from_reader, from_slice, from_str, to_string, to_writer,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Config {
    name: String,
    enabled: bool,
    count: u128,
    ratio: f64,
    optional: Option<String>,
    values: Vec<i32>,
    labels: BTreeMap<String, String>,
}

#[test]
fn structs_round_trip_through_serde() {
    let value = Config {
        name: "true".to_owned(),
        enabled: true,
        count: u128::MAX,
        ratio: -0.0,
        optional: None,
        values: vec![-1, 0, 2],
        labels: BTreeMap::from([
            ("version".to_owned(), "1.2".to_owned()),
            ("word".to_owned(), "hello".to_owned()),
        ]),
    };

    let yaml = to_string(&value).expect("serialize config");
    assert!(yaml.contains("name: \"true\"\n"));
    assert!(yaml.contains("version: \"1.2\"\n"));
    assert_eq!(
        from_str::<Config>(&yaml).expect("deserialize config"),
        value
    );
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum Mode {
    Unit,
    Newtype(String),
    Tuple(u8, bool),
    Struct { host: String, port: u16 },
}

#[test]
fn enum_shapes_use_yaml_tags_and_round_trip() {
    for value in [
        Mode::Unit,
        Mode::Newtype("value".to_owned()),
        Mode::Tuple(3, true),
        Mode::Struct {
            host: "localhost".to_owned(),
            port: 8080,
        },
    ] {
        let yaml = to_string(&value).expect("serialize enum");
        assert_eq!(from_str::<Mode>(&yaml).expect("deserialize enum"), value);
    }

    assert_eq!(to_string(&Mode::Unit).unwrap(), "Unit\n");
    assert_eq!(
        to_string(&Mode::Newtype("x".into())).unwrap(),
        "!Newtype x\n"
    );
    assert_eq!(
        to_string(&Mode::Tuple(1, false)).unwrap(),
        "!Tuple\n- 1\n- false\n"
    );
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct AttributeConfig {
    #[serde(rename = "display-name", alias = "name")]
    display_name: String,
    #[serde(default)]
    retries: u8,
    #[serde(flatten)]
    extra: BTreeMap<String, String>,
    #[serde(skip)]
    cached: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    omitted: Option<String>,
}

#[test]
fn serde_attributes_are_honored_by_standard_derives() {
    let value: AttributeConfig = from_str("name: app\ncustom: kept\n").unwrap();
    assert_eq!(value.display_name, "app");
    assert_eq!(value.retries, 0);
    assert_eq!(value.cached, 0);
    assert_eq!(value.omitted, None);
    assert_eq!(value.extra.get("custom").map(String::as_str), Some("kept"));

    let yaml = to_string(&value).unwrap();
    assert!(yaml.contains("display-name: app\n"));
    assert!(!yaml.contains("cached:"));
    assert!(!yaml.contains("omitted:"));
}

mod string_u16 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &u16, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format!("port-{value}"))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<u16, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value
            .strip_prefix("port-")
            .ok_or_else(|| serde::de::Error::custom("expected port prefix"))?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct CustomHook {
    #[serde(with = "string_u16")]
    port: u16,
}

#[test]
fn custom_serde_hooks_use_the_adapter_normally() {
    let value = CustomHook { port: 8080 };
    let yaml = to_string(&value).unwrap();
    assert_eq!(yaml, "port: port-8080\n");
    assert_eq!(from_str::<CustomHook>(&yaml).unwrap(), value);
}

#[test]
fn all_io_entry_points_work() {
    let value = vec![1_u16, 2, 3];
    let yaml = to_string(&value).unwrap();
    assert_eq!(from_slice::<Vec<u16>>(yaml.as_bytes()).unwrap(), value);
    assert_eq!(
        from_reader::<_, Vec<u16>>(Cursor::new(yaml.as_bytes())).unwrap(),
        value
    );

    let mut output = Vec::new();
    to_writer(&mut output, &value).unwrap();
    assert_eq!(output, yaml.as_bytes());

    let direct = Deserializer::from_reader(Cursor::new(yaml));
    assert_eq!(Vec::<u16>::deserialize(direct).unwrap(), value);
}

#[test]
fn serializer_and_deserializer_support_multiple_documents() {
    let mut output = Vec::new();
    let mut serializer = Serializer::new(&mut output);
    1_u8.serialize(&mut serializer).unwrap();
    vec![2_u8, 3].serialize(&mut serializer).unwrap();
    serializer.flush().unwrap();
    assert_eq!(output, b"1\n---\n- 2\n- 3\n");

    let mut deserializer = Deserializer::from_slice(&output);
    let first = u8::deserialize(deserializer.next().unwrap()).unwrap();
    let second = Vec::<u8>::deserialize(deserializer.next().unwrap()).unwrap();
    assert_eq!(first, 1);
    assert_eq!(second, vec![2, 3]);
    assert!(deserializer.next().is_none());
    assert!(from_slice::<u8>(&output).is_err());
}

#[derive(Debug, PartialEq, Deserialize)]
struct Borrowed<'a> {
    value: &'a str,
}

#[test]
fn plain_strings_can_borrow_from_input() {
    let input = "value: borrowed\n".to_owned();
    let borrowed: Borrowed<'_> = from_str(&input).unwrap();
    assert_eq!(borrowed.value, "borrowed");
    assert!(std::ptr::eq(
        borrowed.value.as_ptr(),
        input[7..].trim_end().as_ptr()
    ));
}

#[derive(Debug, PartialEq, Deserialize)]
struct AliasConfig {
    original: Vec<u8>,
    copy: Vec<u8>,
}

#[test]
fn aliases_deserialize_as_their_anchored_values() {
    let value: AliasConfig = from_str("original: &items [1, 2]\ncopy: *items\n").unwrap();
    assert_eq!(value.original, vec![1, 2]);
    assert_eq!(value.copy, vec![1, 2]);

    let error = from_str::<AliasConfig>("original: [1]\ncopy: *missing\n").unwrap_err();
    assert!(error.to_string().contains("unknown anchor"));
    assert!(error.location().is_some());
}

#[test]
fn core_schema_and_explicit_tags_are_resolved() {
    assert_eq!(from_str::<Option<String>>("null\n").unwrap(), None);
    assert!(from_str::<bool>("TRUE\n").unwrap());
    assert_eq!(from_str::<u16>("0x10\n").unwrap(), 16);
    assert_eq!(from_str::<String>("1.2\n").unwrap(), "1.2");
    assert_eq!(from_str::<String>("!!str 42\n").unwrap(), "42");
    assert!(from_str::<f64>(".inf\n").unwrap().is_infinite());
}

#[test]
fn collection_keys_use_explicit_key_syntax() {
    let value = BTreeMap::from([(vec![1_u8, 2], "pair".to_owned())]);
    let yaml = to_string(&value).unwrap();
    assert!(yaml.starts_with("?\n  - 1\n  - 2\n: pair\n"));
    assert_eq!(from_str::<BTreeMap<Vec<u8>, String>>(&yaml).unwrap(), value);
}

struct Bytes;

impl Serialize for Bytes {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_bytes(b"bytes")
    }
}

#[test]
fn unsupported_bytes_return_an_error() {
    assert!(to_string(&Bytes).unwrap_err().to_string().contains("bytes"));
}

#[test]
fn deterministic_scalar_and_empty_collection_formatting() {
    assert_eq!(
        to_string(&vec![
            "plain".to_owned(),
            "null".to_owned(),
            "line\nbreak".to_owned(),
            String::new(),
        ])
        .unwrap(),
        "- plain\n- \"null\"\n- \"line\\nbreak\"\n- \"\"\n"
    );
    assert_eq!(to_string(&Vec::<u8>::new()).unwrap(), "[]\n");
    assert_eq!(
        to_string(&BTreeMap::<String, String>::new()).unwrap(),
        "{}\n"
    );
    assert_eq!(to_string(&f64::INFINITY).unwrap(), ".inf\n");
    assert_eq!(to_string(&f64::NEG_INFINITY).unwrap(), "-.inf\n");
    assert_eq!(to_string(&f64::NAN).unwrap(), ".nan\n");
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictConfig {
    #[allow(dead_code)]
    known: bool,
}

#[test]
fn serde_errors_include_paths_and_locations() {
    let error = from_str::<StrictConfig>("known: true\nextra: value\n").unwrap_err();
    assert!(error.to_string().contains("unknown field"));
    assert!(error.location().is_some());

    let error = from_str::<Config>("name: app\nenabled: not-a-bool\n").unwrap_err();
    assert!(error.to_string().contains("enabled"));
    assert_eq!(error.location().unwrap().line(), 2);
}

#[derive(Debug, Deserialize)]
struct Recursive {
    #[allow(dead_code)]
    value: u8,
    #[allow(dead_code)]
    next: Option<Box<Recursive>>,
}

#[test]
fn recursive_aliases_hit_the_recursion_limit() {
    let error = from_str::<Recursive>("&root {value: 1, next: *root}\n").unwrap_err();
    assert!(error.to_string().contains("recursion limit"));
}

struct Expand;

impl<'de> Deserialize<'de> for Expand {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ExpandVisitor;

        impl<'de> serde::de::Visitor<'de> for ExpandVisitor {
            type Value = Expand;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("any recursively expanded YAML value")
            }

            fn visit_unit<E>(self) -> Result<Expand, E> {
                Ok(Expand)
            }

            fn visit_seq<A>(self, mut sequence: A) -> Result<Expand, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                while sequence.next_element::<Expand>()?.is_some() {}
                Ok(Expand)
            }

            fn visit_map<A>(self, mut mapping: A) -> Result<Expand, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                while mapping
                    .next_entry::<serde::de::IgnoredAny, Expand>()?
                    .is_some()
                {}
                Ok(Expand)
            }
        }

        deserializer.deserialize_any(ExpandVisitor)
    }
}

#[test]
fn exponential_alias_expansion_hits_the_repetition_limit() {
    let yaml = concat!(
        "a: &a [null, null, null, null, null, null, null, null, null]\n",
        "b: &b [*a, *a, *a, *a, *a, *a, *a, *a, *a]\n",
        "c: &c [*b, *b, *b, *b, *b, *b, *b, *b, *b]\n",
        "d: &d [*c, *c, *c, *c, *c, *c, *c, *c, *c]\n",
        "e: &e [*d, *d, *d, *d, *d, *d, *d, *d, *d]\n",
        "root: *e\n",
    );
    let error = from_str::<Expand>(yaml).err().expect("expansion must stop");
    assert!(error.to_string().contains("repetition limit"));
}

#[derive(Serialize)]
enum Outer {
    Wrap(Mode),
}

#[test]
fn directly_nested_tagged_enums_are_rejected() {
    let error = to_string(&Outer::Wrap(Mode::Newtype("x".to_owned()))).unwrap_err();
    assert!(error.to_string().contains("nested enums"));
}

#[test]
fn serializer_into_inner_returns_the_writer() {
    let mut serializer = Serializer::new(Vec::new());
    "value".serialize(&mut serializer).unwrap();
    assert_eq!(serializer.into_inner().unwrap(), b"value\n");
}
