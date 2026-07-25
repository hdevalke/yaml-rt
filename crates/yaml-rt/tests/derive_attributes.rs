use std::{
    collections::{BTreeMap, HashMap},
    marker::PhantomData,
};
use yaml_rt::{FromYamlDoc, ToYamlDoc, YamlDoc, YamlRoundTrip};

#[derive(Debug, PartialEq, Eq, YamlRoundTrip)]
struct Config {
    host: String,
    #[yaml(rename = "log-level")]
    log_level: String,
    #[yaml(default = 8080)]
    port: u16,
    /// Enable debug logging.
    #[yaml(default = false)]
    debug: bool,
}

#[test]
fn derive_reads_renamed_fields_and_defaults() {
    let doc = YamlDoc::parse("host: localhost\nlog-level: info\n").expect("valid MVP YAML");

    let config = Config::from_yaml_doc(&doc).expect("derive reads config");

    assert_eq!(
        config,
        Config {
            host: "localhost".to_owned(),
            log_level: "info".to_owned(),
            port: 8080,
            debug: false,
        }
    );
}

#[test]
fn derive_inserts_defaults_with_doc_comments() {
    let mut doc = YamlDoc::parse("host: localhost\nlog-level: info\n").expect("valid MVP YAML");
    let config = Config::from_yaml_doc(&doc).expect("derive reads defaults");

    config
        .apply_to_yaml_doc(&mut doc)
        .expect("derive writes missing defaults");

    assert_eq!(
        doc.to_string(),
        "host: localhost\nlog-level: info\nport: 8080\n# Enable debug logging.\ndebug: false\n"
    );
}

#[derive(Debug, PartialEq, Eq, YamlRoundTrip)]
struct Commented {
    name: String,
    /// Doc comment should lose to yaml comment.
    #[yaml(default = true, comment = "Explicit debug comment.")]
    debug: bool,
}

#[test]
fn yaml_comment_attribute_overrides_doc_comment_for_inserted_fields() {
    let mut doc = YamlDoc::parse("name: app\n").expect("valid MVP YAML");
    let commented = Commented::from_yaml_doc(&doc).expect("derive reads defaults");

    commented
        .apply_to_yaml_doc(&mut doc)
        .expect("derive writes missing default");

    assert_eq!(
        doc.to_string(),
        "name: app\n# Explicit debug comment.\ndebug: true\n"
    );
}

#[derive(Debug, PartialEq, Eq, YamlRoundTrip)]
struct Aliased {
    name: String,
    #[yaml(alias = "legacy-port")]
    port: u16,
}

#[test]
fn alias_attribute_reads_and_updates_legacy_key() {
    let mut doc = YamlDoc::parse("name: app\nlegacy-port: 3000\n").expect("valid MVP YAML");
    let mut aliased = Aliased::from_yaml_doc(&doc).expect("derive reads alias");

    assert_eq!(
        aliased,
        Aliased {
            name: "app".to_owned(),
            port: 3000,
        }
    );

    aliased.port = 9090;
    aliased
        .apply_to_yaml_doc(&mut doc)
        .expect("derive updates alias key");

    assert_eq!(doc.to_string(), "name: app\nlegacy-port: 9090\n");
}

#[test]
fn alias_attribute_inserts_canonical_key_when_missing() {
    let mut doc = YamlDoc::parse("name: app\n").expect("valid MVP YAML");
    let aliased = Aliased {
        name: "app".to_owned(),
        port: 8080,
    };

    aliased
        .apply_to_yaml_doc(&mut doc)
        .expect("derive inserts canonical key");

    assert_eq!(doc.to_string(), "name: app\nport: 8080\n");
}

#[derive(Debug, PartialEq, Eq, YamlRoundTrip)]
struct SkippedField {
    name: String,
    #[yaml(skip)]
    cached_port: u16,
}

#[test]
fn skip_attribute_defaults_field_and_preserves_source_key() {
    let mut doc = YamlDoc::parse("name: app\ncached_port: 3000\n").expect("valid MVP YAML");
    let skipped = SkippedField::from_yaml_doc(&doc).expect("derive reads skipped field");

    assert_eq!(
        skipped,
        SkippedField {
            name: "app".to_owned(),
            cached_port: 0,
        }
    );

    skipped
        .apply_to_yaml_doc(&mut doc)
        .expect("derive writes non-skipped fields");

    assert_eq!(doc.to_string(), "name: app\ncached_port: 3000\n");
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, PartialEq, Eq, YamlRoundTrip)]
struct SkipsSerializingIf {
    name: String,
    #[yaml(
        default = false,
        skip_serializing_if = "is_false",
        comment = "Enable debug logging."
    )]
    debug: bool,
}

#[test]
fn skip_serializing_if_removes_existing_field_when_predicate_is_true() {
    let mut doc = YamlDoc::parse("name: app\ndebug: true\nextra: keep\n").expect("valid MVP YAML");
    let mut config = SkipsSerializingIf::from_yaml_doc(&doc).expect("derive reads config");

    config.debug = false;
    config
        .apply_to_yaml_doc(&mut doc)
        .expect("derive removes skipped field");

    assert_eq!(doc.to_string(), "name: app\nextra: keep\n");
}

#[test]
fn skip_serializing_if_omits_missing_field_and_inserts_when_false() {
    let mut omitted = YamlDoc::parse("name: app\n").expect("valid MVP YAML");
    let config = SkipsSerializingIf {
        name: "app".to_owned(),
        debug: false,
    };

    config
        .apply_to_yaml_doc(&mut omitted)
        .expect("derive omits skipped missing field");

    assert_eq!(omitted.to_string(), "name: app\n");

    let mut inserted = YamlDoc::parse("name: app\n").expect("valid MVP YAML");
    let config = SkipsSerializingIf {
        name: "app".to_owned(),
        debug: true,
    };

    config
        .apply_to_yaml_doc(&mut inserted)
        .expect("derive inserts non-skipped missing field");

    assert_eq!(
        inserted.to_string(),
        "name: app\n# Enable debug logging.\ndebug: true\n"
    );
}

#[derive(Debug, PartialEq, Eq, YamlRoundTrip)]
struct AliasSkippedSerialization {
    name: String,
    #[yaml(
        alias = "legacy-debug",
        default = false,
        skip_serializing_if = "is_false"
    )]
    debug: bool,
}

#[test]
fn skip_serializing_if_removes_existing_alias_field() {
    let mut doc = YamlDoc::parse("name: app\nlegacy-debug: true\n").expect("valid MVP YAML");
    let mut config = AliasSkippedSerialization::from_yaml_doc(&doc).expect("derive reads alias");

    config.debug = false;
    config
        .apply_to_yaml_doc(&mut doc)
        .expect("derive removes alias field");

    assert_eq!(doc.to_string(), "name: app\n");
}

#[derive(Debug, PartialEq, Eq, YamlRoundTrip)]
struct OptionalFields {
    name: String,
    optional: Option<String>,
    #[yaml(default = Some("fallback".to_owned()))]
    defaulted: Option<String>,
    #[yaml(skip_serializing_if = "Option::is_none")]
    omitted: Option<String>,
}

#[test]
fn option_fields_distinguish_missing_defaults_and_explicit_null() {
    let mut doc = YamlDoc::parse("name: app\nomitted: null\n").expect("valid optional YAML");
    let config = OptionalFields::from_yaml_doc(&doc).expect("derive reads optional fields");

    assert_eq!(config.optional, None);
    assert_eq!(config.defaulted.as_deref(), Some("fallback"));
    assert_eq!(config.omitted, None);

    config
        .apply_to_yaml_doc(&mut doc)
        .expect("derive writes optional fields");

    assert_eq!(
        doc.to_string(),
        "name: app\noptional: null\ndefaulted: fallback\n"
    );

    let doc =
        YamlDoc::parse("name: app\noptional: null\ndefaulted: configured\nomitted: present\n")
            .expect("valid present optional YAML");
    let config = OptionalFields::from_yaml_doc(&doc).expect("derive reads present optionals");
    assert_eq!(config.optional, None);
    assert_eq!(config.defaulted.as_deref(), Some("configured"));
    assert_eq!(config.omitted.as_deref(), Some("present"));
}

#[derive(Debug, PartialEq, Eq, YamlRoundTrip)]
struct ServerFields {
    host: String,
    #[yaml(default = 8080)]
    port: u16,
}

#[derive(Debug, PartialEq, Eq, YamlRoundTrip)]
struct FlattenedConfig {
    name: String,
    #[yaml(flatten)]
    server: ServerFields,
}

#[test]
fn flatten_attribute_reads_nested_overlay_from_root_mapping() {
    let doc = YamlDoc::parse("name: app\nhost: localhost\n").expect("valid MVP YAML");

    let config = FlattenedConfig::from_yaml_doc(&doc).expect("derive reads flattened struct");

    assert_eq!(
        config,
        FlattenedConfig {
            name: "app".to_owned(),
            server: ServerFields {
                host: "localhost".to_owned(),
                port: 8080,
            },
        }
    );
}

#[test]
fn flatten_attribute_writes_nested_overlay_to_root_mapping() {
    let mut doc =
        YamlDoc::parse("name: app\nhost: \"localhost\"\nextra: keep\n").expect("valid MVP YAML");
    let mut config = FlattenedConfig::from_yaml_doc(&doc).expect("derive reads flattened struct");

    config.server.host = "example.com".to_owned();
    config.server.port = 9090;
    config
        .apply_to_yaml_doc(&mut doc)
        .expect("derive writes flattened struct");

    assert_eq!(
        doc.to_string(),
        "name: app\nhost: \"example.com\"\nextra: keep\nport: 9090\n"
    );
}

#[derive(Debug, PartialEq, Eq, YamlRoundTrip)]
struct NestedConfig {
    name: String,
    server: ServerFields,
}

#[derive(Debug, PartialEq, Eq, YamlRoundTrip)]
struct NestedCollectionsConfig {
    servers: Vec<ServerFields>,
    groups: BTreeMap<String, ServerFields>,
}

#[derive(Debug, PartialEq, Eq, YamlRoundTrip)]
struct FlowNestedConfig {
    servers: Vec<ServerFields>,
}

#[derive(Debug, PartialEq, Eq, YamlRoundTrip)]
struct StandardShapeConfig {
    primary: Box<ServerFields>,
    fixed: [ServerFields; 2],
    by_name: HashMap<String, ServerFields>,
}

#[derive(Debug, PartialEq, Eq, YamlRoundTrip)]
struct GenericLeaf<T> {
    value: T,
}

#[derive(Debug, PartialEq, Eq, YamlRoundTrip)]
struct GenericConfig<'a, T, const N: usize>
where
    T: Copy,
{
    current: T,
    history: Vec<T>,
    fixed: [T; N],
    nested: Vec<GenericLeaf<T>>,
    #[yaml(skip)]
    marker: PhantomData<&'a T>,
}

#[derive(Debug, PartialEq, Eq, YamlRoundTrip)]
struct GenericFlatten<T> {
    #[yaml(flatten)]
    values: T,
}

#[test]
fn derived_structs_work_inside_box_arrays_and_hash_maps() {
    let doc = YamlDoc::parse(
        "primary:\n  host: one\n  port: 1\nfixed:\n  -\n    host: two\n    port: 2\n  -\n    host: three\n    port: 3\nby_name:\n  z:\n    host: last\n    port: 4\n",
    )
    .expect("valid standard shape YAML");
    let config = StandardShapeConfig::from_yaml_doc(&doc).expect("derive reads standard shapes");

    assert_eq!(config.primary.host, "one");
    assert_eq!(config.fixed[1].host, "three");
    assert_eq!(config.by_name.get("z").map(|server| server.port), Some(4));
}

#[test]
fn derive_preserves_generics_const_generics_and_where_clauses() {
    let mut doc =
        YamlDoc::parse("current: 1\nhistory: [2, 3]\nfixed: [4, 5]\nnested:\n  - value: 6\n")
            .expect("valid generic config YAML");
    let mut config =
        GenericConfig::<'static, u16, 2>::from_yaml_doc(&doc).expect("generic derive reads");

    assert_eq!(config.current, 1);
    assert_eq!(config.history, vec![2, 3]);
    assert_eq!(config.fixed, [4, 5]);
    assert_eq!(config.nested[0].value, 6);

    config.current = 7;
    config.fixed[1] = 8;
    config.nested[0].value = 9;
    config
        .apply_to_yaml_doc(&mut doc)
        .expect("generic derive writes");

    assert_eq!(
        doc.to_string(),
        "current: 7\nhistory: [2, 3]\nfixed: [4, 8]\nnested:\n  - value: 9\n"
    );
}

#[test]
fn derive_infers_flatten_bounds_for_generic_fields() {
    let mut doc = YamlDoc::parse("host: one\nport: 1\n").expect("valid flattened generic YAML");
    let mut config =
        GenericFlatten::<ServerFields>::from_yaml_doc(&doc).expect("generic flatten derive reads");

    config.values.port = 2;
    config
        .apply_to_yaml_doc(&mut doc)
        .expect("generic flatten derive writes");

    assert_eq!(doc.to_string(), "host: one\nport: 2\n");
}

#[test]
fn nested_struct_sequence_patches_flow_mappings_in_place() {
    let mut doc = YamlDoc::parse("{servers: [{host: \"one\", extra: keep}], tail: yes}\n")
        .expect("valid flow YAML");
    let mut config = FlowNestedConfig::from_yaml_doc(&doc).expect("derive reads flow mappings");

    config.servers[0].host = "updated".to_owned();
    config.servers.push(ServerFields {
        host: "two".to_owned(),
        port: 9090,
    });
    config
        .apply_to_yaml_doc(&mut doc)
        .expect("derive patches flow mappings");

    assert_eq!(
        doc.to_string(),
        "{servers: [{host: \"updated\", extra: keep, port: 8080}, {host: two, port: 9090}], tail: yes}\n"
    );

    doc.commit_edits().expect("flow update commits");
    let mut config = FlowNestedConfig::from_yaml_doc(&doc).expect("derive rereads flow mappings");
    config.servers.truncate(1);
    config
        .apply_to_yaml_doc(&mut doc)
        .expect("derive shrinks flow sequence");

    assert_eq!(
        doc.to_string(),
        "{servers: [{host: \"updated\", extra: keep, port: 8080}], tail: yes}\n"
    );
}

#[test]
fn nested_structs_work_inside_sequences_and_mappings() {
    let mut doc = YamlDoc::parse(
        "servers:\n  -\n    host: \"one\" # keep\n    extra: keep\ngroups:\n  primary:\n    host: old\n",
    )
    .expect("valid nested collection YAML");
    let mut config =
        NestedCollectionsConfig::from_yaml_doc(&doc).expect("derive reads nested collections");

    config.servers[0].host = "updated".to_owned();
    config.servers.push(ServerFields {
        host: "two".to_owned(),
        port: 9090,
    });
    config
        .groups
        .get_mut("primary")
        .expect("primary group")
        .port = 9443;
    config.groups.insert(
        "secondary".to_owned(),
        ServerFields {
            host: "backup".to_owned(),
            port: 8081,
        },
    );
    config
        .apply_to_yaml_doc(&mut doc)
        .expect("derive writes nested collections");

    assert_eq!(
        doc.to_string(),
        "servers:\n  -\n    host: \"updated\" # keep\n    extra: keep\n    port: 8080\n  -\n    host: two\n    port: 9090\ngroups:\n  primary:\n    host: old\n    port: 9443\n  secondary:\n    host: backup\n    port: 8081\n"
    );
}

#[test]
fn nested_struct_field_reads_and_updates_existing_mapping() {
    let mut doc = YamlDoc::parse("name: app\nserver:\n  host: \"localhost\"\n  extra: keep\n")
        .expect("valid nested YAML");
    let mut config = NestedConfig::from_yaml_doc(&doc).expect("derive reads nested config");

    assert_eq!(
        config.server,
        ServerFields {
            host: "localhost".to_owned(),
            port: 8080,
        }
    );

    config.server.host = "example.com".to_owned();
    config.server.port = 9090;
    config
        .apply_to_yaml_doc(&mut doc)
        .expect("derive writes nested config");

    assert_eq!(
        doc.to_string(),
        "name: app\nserver:\n  host: \"example.com\"\n  extra: keep\n  port: 9090\n"
    );
}

#[test]
fn nested_struct_field_preserves_comments_and_unknown_siblings() {
    let mut doc = YamlDoc::parse(
        "name: app\nserver:\n  # selected host\n  host: \"localhost\" # inline\n  extra: keep\n",
    )
    .expect("valid nested YAML");
    let mut config = NestedConfig::from_yaml_doc(&doc).expect("derive reads nested config");

    config.server.host = "example.com".to_owned();
    config.server.port = 9090;
    config
        .apply_to_yaml_doc(&mut doc)
        .expect("derive writes nested config");

    assert_eq!(
        doc.to_string(),
        "name: app\nserver:\n  # selected host\n  host: \"example.com\" # inline\n  extra: keep\n  port: 9090\n"
    );
}

#[test]
fn nested_struct_field_inserts_missing_mapping() {
    let mut doc = YamlDoc::parse("name: app\n").expect("valid YAML");
    let config = NestedConfig {
        name: "app".to_owned(),
        server: ServerFields {
            host: "localhost".to_owned(),
            port: 8080,
        },
    };

    config
        .apply_to_yaml_doc(&mut doc)
        .expect("derive inserts nested config");

    assert_eq!(
        doc.to_string(),
        "name: app\nserver:\n  host: localhost\n  port: 8080\n"
    );
}

#[derive(Debug, PartialEq, Eq, YamlRoundTrip)]
#[yaml(insert_order = "struct")]
struct OrderedNestedConfig {
    name: String,
    /// Server settings.
    server: ServerFields,
    tail: String,
}

#[test]
fn nested_struct_field_inserts_with_comment_and_struct_order() {
    let mut doc = YamlDoc::parse("name: app\ntail: keep\n").expect("valid YAML");
    let config = OrderedNestedConfig {
        name: "app".to_owned(),
        server: ServerFields {
            host: "localhost".to_owned(),
            port: 8080,
        },
        tail: "keep".to_owned(),
    };

    config
        .apply_to_yaml_doc(&mut doc)
        .expect("derive inserts ordered nested config");

    assert_eq!(
        doc.to_string(),
        "name: app\n# Server settings.\nserver:\n  host: localhost\n  port: 8080\ntail: keep\n"
    );
}

#[derive(Debug, PartialEq, Eq, YamlRoundTrip)]
struct CollectionConfig {
    name: String,
    #[yaml(default)]
    ports: Vec<u16>,
    #[yaml(default)]
    limits: BTreeMap<String, u16>,
}

#[test]
fn derive_inserts_missing_collection_fields() {
    let mut doc = YamlDoc::parse("name: app\n").expect("valid YAML");
    let mut limits = BTreeMap::new();
    limits.insert("high".to_owned(), 5);
    limits.insert("low".to_owned(), 1);
    let config = CollectionConfig {
        name: "app".to_owned(),
        ports: vec![8080, 9090],
        limits,
    };

    config
        .apply_to_yaml_doc(&mut doc)
        .expect("derive inserts collections");

    assert_eq!(
        doc.to_string(),
        "name: app\nports:\n  - 8080\n  - 9090\nlimits:\n  high: 5\n  low: 1\n"
    );
}

#[test]
fn repeated_apply_can_commit_between_passes() {
    let mut doc = YamlDoc::parse("name: app\n").expect("valid YAML");
    let config = CollectionConfig {
        name: "app".to_owned(),
        ports: vec![8080],
        limits: BTreeMap::new(),
    };

    config
        .apply_to_yaml_doc(&mut doc)
        .expect("first apply inserts ports");
    doc.commit_edits().expect("commit inserted fields");

    let mut config = CollectionConfig::from_yaml_doc(&doc).expect("derive reads committed fields");
    config.ports = vec![9090];
    config
        .apply_to_yaml_doc(&mut doc)
        .expect("second apply updates committed field");

    assert_eq!(doc.to_string(), "name: app\nports:\n  - 9090\nlimits: {}\n");
}

#[derive(Debug, PartialEq, Eq, YamlRoundTrip)]
#[yaml(prune_unknown_fields)]
struct PrunedConfig {
    name: String,
    #[yaml(alias = "legacy-port", default = 8080)]
    port: u16,
}

#[test]
fn prune_unknown_fields_removes_unmodeled_root_entries_after_write() {
    let mut doc =
        YamlDoc::parse("name: app\nlegacy-port: 3000\nextra: remove-me\n").expect("valid MVP YAML");
    let mut config = PrunedConfig::from_yaml_doc(&doc).expect("derive reads config");

    config.port = 9090;
    config
        .apply_to_yaml_doc(&mut doc)
        .expect("derive prunes unknown fields");

    assert_eq!(doc.to_string(), "name: app\nlegacy-port: 9090\n");
}

#[derive(Debug, PartialEq, Eq, YamlRoundTrip)]
#[yaml(preserve_unknown_fields)]
struct ExplicitlyPreservedConfig {
    name: String,
}

#[test]
fn preserve_unknown_fields_struct_attribute_keeps_default_behavior() {
    let mut doc = YamlDoc::parse("name: app\nextra: keep-me\n").expect("valid MVP YAML");
    let config = ExplicitlyPreservedConfig::from_yaml_doc(&doc).expect("derive reads config");

    config
        .apply_to_yaml_doc(&mut doc)
        .expect("derive preserves unknown fields");

    assert_eq!(doc.to_string(), "name: app\nextra: keep-me\n");
}

#[derive(Debug, PartialEq, Eq, YamlRoundTrip)]
#[yaml(insert_order = "struct")]
struct StructOrderedConfig {
    host: String,
    #[yaml(default = 8080)]
    port: u16,
    extra: String,
}

#[test]
fn insert_order_struct_inserts_missing_fields_before_next_declared_entry() {
    let mut doc = YamlDoc::parse("host: localhost\nextra: keep\n").expect("valid MVP YAML");
    let config = StructOrderedConfig::from_yaml_doc(&doc).expect("derive reads defaults");

    config
        .apply_to_yaml_doc(&mut doc)
        .expect("derive writes missing default in struct order");

    assert_eq!(
        doc.to_string(),
        "host: localhost\nport: 8080\nextra: keep\n"
    );
}

#[derive(Debug, PartialEq, Eq, YamlRoundTrip)]
#[yaml(insert_order = "append")]
struct AppendOrderedConfig {
    host: String,
    #[yaml(default = 8080)]
    port: u16,
    extra: String,
}

#[test]
fn insert_order_append_keeps_default_append_behavior() {
    let mut doc = YamlDoc::parse("host: localhost\nextra: keep\n").expect("valid MVP YAML");
    let config = AppendOrderedConfig::from_yaml_doc(&doc).expect("derive reads defaults");

    config
        .apply_to_yaml_doc(&mut doc)
        .expect("derive appends missing default");

    assert_eq!(
        doc.to_string(),
        "host: localhost\nextra: keep\nport: 8080\n"
    );
}
