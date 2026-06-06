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
