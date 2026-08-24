use std::{
    collections::{BTreeMap, HashMap},
    marker::PhantomData,
    time::Duration,
};
use yaml_rt::{FromYamlDoc, ToYamlDoc, YamlDoc, YamlRt};

#[derive(Debug, PartialEq, Eq, YamlRt)]
#[yaml(rename_all = "camelCase")]
struct RenamedFields {
    first_value: String,
    #[yaml(rename = "fixed", alias = "legacy")]
    second_value: u16,
}

#[test]
fn struct_rename_all_reads_patches_and_inserts_with_explicit_rename_precedence() {
    let mut doc = YamlDoc::parse("firstValue: old\nlegacy: 1\n").expect("valid renamed fields");
    let mut value = RenamedFields::from_yaml_doc(&doc).expect("renamed fields read");
    assert_eq!(value.first_value, "old");
    assert_eq!(value.second_value, 1);

    value.first_value = "new".to_owned();
    value.second_value = 2;
    value
        .apply_to_yaml_doc(&mut doc)
        .expect("renamed fields patch");
    assert_eq!(doc.to_string(), "firstValue: new\nlegacy: 2\n");

    let mut missing = YamlDoc::parse("{}\n").expect("valid empty mapping");
    value
        .apply_to_yaml_doc(&mut missing)
        .expect("renamed fields insert");
    assert_eq!(missing.to_string(), "{firstValue: new, fixed: 2}\n");
}

#[test]
fn struct_rename_all_supports_every_documented_field_case() {
    macro_rules! assert_key {
        ($name:ident, $rule:literal, $key:literal) => {{
            #[derive(YamlRt)]
            #[yaml(rename_all = $rule)]
            struct $name {
                http_server_id: u8,
            }
            let mut doc = YamlDoc::parse("{}\n").unwrap();
            $name { http_server_id: 1 }
                .apply_to_yaml_doc(&mut doc)
                .unwrap();
            assert_eq!(doc.to_string(), concat!("{", $key, ": 1}\n"));
        }};
    }

    assert_key!(Lower, "lowercase", "http_server_id");
    assert_key!(Snake, "snake_case", "http_server_id");
    assert_key!(Kebab, "kebab-case", "http-server-id");
    assert_key!(Screaming, "SCREAMING_SNAKE_CASE", "HTTP_SERVER_ID");
    assert_key!(Camel, "camelCase", "httpServerId");
    assert_key!(Pascal, "PascalCase", "HttpServerId");
}

mod duration_seconds {
    use std::time::Duration;
    use yaml_rt::YamlError;

    pub type Repr = u64;

    #[expect(
        clippy::unnecessary_wraps,
        reason = "yaml(with) adapters require fallible conversion signatures"
    )]
    pub fn from_yaml(value: Repr) -> Result<Duration, YamlError> {
        Ok(Duration::from_secs(value))
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "yaml(with) adapters require fallible conversion signatures"
    )]
    pub fn to_yaml(value: &Duration) -> Result<Repr, YamlError> {
        Ok(value.as_secs())
    }
}

#[derive(Debug, PartialEq, Eq)]
struct PortSet(Vec<u16>);

mod port_list {
    use super::PortSet;
    use yaml_rt::YamlError;

    pub type Repr = Vec<u16>;

    #[expect(
        clippy::unnecessary_wraps,
        reason = "yaml(with) adapters require fallible conversion signatures"
    )]
    pub fn from_yaml(value: Repr) -> Result<PortSet, YamlError> {
        Ok(PortSet(value))
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "yaml(with) adapters require fallible conversion signatures"
    )]
    pub fn to_yaml(value: &PortSet) -> Result<Repr, YamlError> {
        Ok(value.0.clone())
    }
}

#[derive(Debug, PartialEq, Eq)]
struct PositivePort(u16);

mod positive_port {
    use super::PositivePort;
    use yaml_rt::{Diagnostic, DiagnosticKind, Span, YamlError};

    pub type Repr = u16;

    pub fn from_yaml(value: Repr) -> Result<PositivePort, YamlError> {
        if value == 0 {
            return Err(YamlError::new(Diagnostic::new(
                DiagnosticKind::Semantic,
                "port must be positive",
                Span::empty(0),
            )));
        }
        Ok(PositivePort(value))
    }

    pub fn to_yaml(value: &PositivePort) -> Result<Repr, YamlError> {
        if value.0 == 0 {
            return Err(YamlError::new(Diagnostic::new(
                DiagnosticKind::Emitter,
                "port must be positive",
                Span::empty(0),
            )));
        }
        Ok(value.0)
    }
}

#[derive(Debug, PartialEq, Eq, YamlRt)]
struct AdapterConfig {
    #[yaml(
        with = "duration_seconds",
        rename = "timeout-seconds",
        alias = "timeout",
        default = Duration::from_secs(30),
        comment = "Request timeout in seconds."
    )]
    timeout: Duration,
    #[yaml(with = "port_list")]
    ports: PortSet,
    #[yaml(with = "positive_port")]
    admin_port: PositivePort,
    #[yaml(
        with = "duration_seconds",
        default = Duration::from_secs(0),
        skip_serializing_if = "Duration::is_zero"
    )]
    cooldown: Duration,
}

#[test]
fn with_adapters_patch_scalar_and_collection_representations_in_flow_yaml() {
    let mut doc = YamlDoc::parse("{timeout: 5, ports: [80, 443], admin_port: 9000, tail: keep}\n")
        .expect("valid adapter YAML");
    let mut config = AdapterConfig::from_yaml_doc(&doc).expect("adapters read representations");

    assert_eq!(config.timeout, Duration::from_secs(5));
    assert_eq!(config.ports, PortSet(vec![80, 443]));
    assert_eq!(config.cooldown, Duration::from_secs(0));

    config.timeout = Duration::from_secs(6);
    config.ports.0[0] = 81;
    config.ports.0.push(8080);
    config.admin_port = PositivePort(9001);
    config
        .apply_to_yaml_doc(&mut doc)
        .expect("adapters write representations");

    assert_eq!(
        doc.to_string(),
        "{timeout: 6, ports: [81, 443, 8080], admin_port: 9001, tail: keep}\n"
    );
}

#[test]
fn with_adapters_apply_defaults_comments_and_omission_normally() {
    let mut doc =
        YamlDoc::parse("ports: [80]\nadmin_port: 9000\n").expect("valid adapter defaults YAML");
    let config = AdapterConfig::from_yaml_doc(&doc).expect("adapter default is applied");

    assert_eq!(config.timeout, Duration::from_secs(30));
    config
        .apply_to_yaml_doc(&mut doc)
        .expect("adapter default is emitted");

    assert_eq!(
        doc.to_string(),
        "ports: [80]\nadmin_port: 9000\n# Request timeout in seconds.\ntimeout-seconds: 30\n"
    );

    let mut doc =
        YamlDoc::parse("timeout-seconds: 30\nports: [80]\nadmin_port: 9000\ncooldown: 2\n")
            .expect("valid adapter omission YAML");
    let mut config = AdapterConfig::from_yaml_doc(&doc).expect("adapter reads omitted field");
    config.cooldown = Duration::from_secs(0);
    config
        .apply_to_yaml_doc(&mut doc)
        .expect("adapter omission removes field");

    assert_eq!(
        doc.to_string(),
        "timeout-seconds: 30\nports: [80]\nadmin_port: 9000\n"
    );
}

#[test]
fn with_adapters_propagate_read_and_write_conversion_errors() {
    let doc = YamlDoc::parse("timeout: 5\nports: [80]\nadmin_port: 0\n")
        .expect("valid representation YAML");
    let error = AdapterConfig::from_yaml_doc(&doc).expect_err("zero port conversion must fail");
    assert_eq!(error.diagnostic.message, "port must be positive");

    let mut doc =
        YamlDoc::parse("timeout: 5\nports: [80]\nadmin_port: 1\n").expect("valid adapter YAML");
    let mut config = AdapterConfig::from_yaml_doc(&doc).expect("valid port conversion");
    config.admin_port = PositivePort(0);
    let error = config
        .apply_to_yaml_doc(&mut doc)
        .expect_err("invalid typed port must fail conversion");
    assert_eq!(error.diagnostic.message, "port must be positive");
}

#[derive(Debug, PartialEq, Eq, YamlRt)]
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

#[derive(Debug, PartialEq, Eq, YamlRt)]
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

#[derive(Debug, PartialEq, Eq, YamlRt)]
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

#[derive(Debug, PartialEq, Eq, YamlRt)]
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

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "yaml(skip_serializing_if) predicates receive a reference to the field"
)]
fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, PartialEq, Eq, YamlRt)]
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

#[derive(Debug, PartialEq, Eq, YamlRt)]
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

#[derive(Debug, PartialEq, Eq, YamlRt)]
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

#[derive(Debug, PartialEq, Eq, YamlRt)]
struct ServerFields {
    host: String,
    #[yaml(default = 8080)]
    port: u16,
}

#[derive(Debug, PartialEq, Eq, YamlRt)]
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

type ExtraValues = BTreeMap<String, String>;

#[derive(Debug, PartialEq, Eq, YamlRt)]
struct CatchAllConfig {
    #[yaml(alias = "legacy-name")]
    name: String,
    #[yaml(skip)]
    cached: u16,
    #[yaml(default = false, skip_serializing_if = "is_false")]
    debug: bool,
    #[yaml(flatten)]
    extra: ExtraValues,
}

#[test]
fn flattened_btree_map_owns_only_unclaimed_block_entries() {
    let input = "legacy-name: app\ncached: 7\ndebug: true\nquoted: \"old\" # keep\nremove: gone\n";
    let mut doc = YamlDoc::parse(input).expect("valid catch-all mapping");
    let mut config = CatchAllConfig::from_yaml_doc(&doc).expect("catch-all map reads");

    assert_eq!(
        config.extra,
        BTreeMap::from([
            ("quoted".to_owned(), "old".to_owned()),
            ("remove".to_owned(), "gone".to_owned()),
        ])
    );
    config
        .apply_to_yaml_doc(&mut doc)
        .expect("unchanged catch-all map writes");
    assert_eq!(doc.to_string(), input);

    config.debug = false;
    config.extra.insert("quoted".to_owned(), "new".to_owned());
    config.extra.remove("remove");
    config.extra.insert("zeta".to_owned(), "last".to_owned());
    config.extra.insert("alpha".to_owned(), "first".to_owned());
    config
        .apply_to_yaml_doc(&mut doc)
        .expect("catch-all map synchronizes");

    assert_eq!(
        doc.to_string(),
        "legacy-name: app\ncached: 7\nquoted: \"new\" # keep\nalpha: first\nzeta: last\n"
    );
}

#[test]
fn flattened_btree_map_patches_flow_mappings_and_reparses() {
    #[derive(Debug, PartialEq, Eq, YamlRt)]
    struct FlowConfig {
        name: String,
        #[yaml(flatten)]
        extra: BTreeMap<String, String>,
    }

    let mut doc = YamlDoc::parse("{name: app, beta: \"old\", remove: gone}\n")
        .expect("valid flow catch-all mapping");
    let mut config = FlowConfig::from_yaml_doc(&doc).expect("flow catch-all reads");
    config.extra.insert("beta".to_owned(), "new".to_owned());
    config.extra.remove("remove");
    config.extra.insert("alpha".to_owned(), "first".to_owned());
    config
        .apply_to_yaml_doc(&mut doc)
        .expect("flow catch-all writes");

    assert_eq!(
        doc.to_string(),
        "{name: app, beta: \"new\", alpha: first}\n"
    );
    doc.commit_edits().expect("flow catch-all output reparses");
    assert_eq!(
        FlowConfig::from_yaml_doc(&doc)
            .expect("flow catch-all rereads")
            .extra,
        BTreeMap::from([
            ("alpha".to_owned(), "first".to_owned()),
            ("beta".to_owned(), "new".to_owned()),
        ])
    );
}

#[derive(Debug, PartialEq, Eq, YamlRt)]
struct FlattenedServerAndExtras {
    name: String,
    #[yaml(flatten)]
    server: ServerFields,
    #[yaml(flatten)]
    extra: BTreeMap<String, String>,
}

#[test]
fn catch_all_map_composes_with_flattened_structs_and_rejects_collisions() {
    let input = "name: app\nhost: localhost\ncustom: keep\n";
    let mut doc = YamlDoc::parse(input).expect("valid composed flatten mapping");
    let mut config = FlattenedServerAndExtras::from_yaml_doc(&doc).expect("composed flatten reads");

    assert_eq!(config.extra.get("custom").map(String::as_str), Some("keep"));
    assert!(!config.extra.contains_key("name"));
    assert!(!config.extra.contains_key("host"));

    config
        .extra
        .insert("host".to_owned(), "collision".to_owned());
    let error = config
        .apply_to_yaml_doc(&mut doc)
        .expect_err("modeled-key collision fails");
    assert!(error.diagnostic.message.contains("modeled key `host`"));
    assert_eq!(doc.to_string(), input);
}

#[derive(Debug, PartialEq, Eq, YamlRt)]
struct ServerWithExtras {
    host: String,
    #[yaml(flatten)]
    extra: BTreeMap<String, String>,
}

#[derive(Debug, PartialEq, Eq, YamlRt)]
struct OuterFlattenedExtras {
    name: String,
    #[yaml(flatten)]
    server: ServerWithExtras,
}

#[test]
fn nested_catch_all_excludes_outer_modeled_keys() {
    let doc = YamlDoc::parse("name: app\nhost: localhost\ncustom: keep\n")
        .expect("valid recursively flattened mapping");
    let config = OuterFlattenedExtras::from_yaml_doc(&doc).expect("nested catch-all reads");

    assert_eq!(
        config.server.extra,
        BTreeMap::from([("custom".to_owned(), "keep".to_owned())])
    );
}

type HashedExtras = HashMap<String, String>;

#[test]
fn generic_flatten_supports_hash_map_aliases_with_sorted_insertions() {
    let mut doc = YamlDoc::parse("existing: keep\n").expect("valid generic catch-all mapping");
    let mut config =
        GenericFlatten::<HashedExtras>::from_yaml_doc(&doc).expect("generic catch-all reads");
    config.values.insert("zeta".to_owned(), "last".to_owned());
    config.values.insert("alpha".to_owned(), "first".to_owned());
    config
        .apply_to_yaml_doc(&mut doc)
        .expect("generic catch-all writes");

    assert_eq!(
        doc.to_string(),
        "existing: keep\nalpha: first\nzeta: last\n"
    );
}

#[derive(Debug, PartialEq, Eq, YamlRt)]
struct AmbiguousCatchAll {
    #[yaml(flatten)]
    ordered: BTreeMap<String, String>,
    #[yaml(flatten)]
    hashed: HashMap<String, String>,
}

#[test]
fn multiple_catch_all_maps_fail_before_reading_or_writing() {
    let input = "extra: keep\n";
    let doc = YamlDoc::parse(input).expect("valid ambiguous catch-all mapping");
    let read_error =
        AmbiguousCatchAll::from_yaml_doc(&doc).expect_err("multiple catch-all maps reject read");
    assert!(read_error.diagnostic.message.contains("at most one"));

    let mut doc = YamlDoc::parse(input).expect("valid ambiguous catch-all mapping");
    let value = AmbiguousCatchAll {
        ordered: BTreeMap::new(),
        hashed: HashMap::new(),
    };
    let write_error = value
        .apply_to_yaml_doc(&mut doc)
        .expect_err("multiple catch-all maps reject write");
    assert!(write_error.diagnostic.message.contains("at most one"));
    assert_eq!(doc.to_string(), input);
}

#[derive(Debug, PartialEq, Eq, YamlRt)]
struct NestedConfig {
    name: String,
    server: ServerFields,
}

#[derive(Debug, PartialEq, Eq, YamlRt)]
struct NestedCollectionsConfig {
    servers: Vec<ServerFields>,
    groups: BTreeMap<String, ServerFields>,
}

#[derive(Debug, PartialEq, Eq, YamlRt)]
struct FlowNestedConfig {
    servers: Vec<ServerFields>,
}

#[derive(Debug, PartialEq, Eq, YamlRt)]
struct StandardShapeConfig {
    primary: Box<ServerFields>,
    fixed: [ServerFields; 2],
    maybe: Option<ServerFields>,
    by_name: HashMap<String, ServerFields>,
}

#[derive(Debug, PartialEq, Eq, YamlRt)]
struct GenericLeaf<T> {
    value: T,
}

#[derive(Debug, PartialEq, Eq, YamlRt)]
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

#[derive(Debug, PartialEq, Eq, YamlRt)]
struct GenericFlatten<T> {
    #[yaml(flatten)]
    values: T,
}

#[test]
fn derived_structs_work_inside_box_arrays_and_hash_maps() {
    let doc = YamlDoc::parse(
        "primary:\n  host: one\n  port: 1\nfixed:\n  -\n    host: two\n    port: 2\n  -\n    host: three\n    port: 3\nmaybe:\n  host: optional\n  port: 4\nby_name:\n  z:\n    host: last\n    port: 5\n",
    )
    .expect("valid standard shape YAML");
    let config = StandardShapeConfig::from_yaml_doc(&doc).expect("derive reads standard shapes");

    assert_eq!(config.primary.host, "one");
    assert_eq!(config.fixed[1].host, "three");
    assert_eq!(
        config.maybe.as_ref().map(|server| server.host.as_str()),
        Some("optional")
    );
    assert_eq!(config.by_name.get("z").map(|server| server.port), Some(5));
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
fn nested_struct_sequence_preserves_and_resizes_block_yaml_incrementally() {
    let input = "servers:\n  -\n    host: \"one\" # keep\n    port: 8080\n    extra: keep\n  -\n    host: two\n    port: 9090\ntail: yes\n";
    let mut doc = YamlDoc::parse(input).expect("valid block YAML");
    let mut config = FlowNestedConfig::from_yaml_doc(&doc).expect("derive reads block mappings");

    config
        .apply_to_yaml_doc(&mut doc)
        .expect("unchanged derive apply succeeds");
    assert_eq!(doc.to_string(), input);

    config.servers[0].host = "updated".to_owned();
    config.servers.truncate(1);
    config
        .apply_to_yaml_doc(&mut doc)
        .expect("derive patches and shrinks block sequence");
    assert_eq!(
        doc.to_string(),
        "servers:\n  -\n    host: \"updated\" # keep\n    port: 8080\n    extra: keep\ntail: yes\n"
    );

    doc.commit_edits().expect("block shrink commits");
    config.servers.push(ServerFields {
        host: "new".to_owned(),
        port: 9443,
    });
    config
        .apply_to_yaml_doc(&mut doc)
        .expect("derive grows block sequence");
    assert_eq!(
        doc.to_string(),
        "servers:\n  -\n    host: \"updated\" # keep\n    port: 8080\n    extra: keep\n  -\n    host: new\n    port: 9443\ntail: yes\n"
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

#[derive(Debug, PartialEq, Eq, YamlRt)]
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

#[derive(Debug, PartialEq, Eq, YamlRt)]
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

#[derive(Debug, PartialEq, Eq, YamlRt)]
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

#[derive(Debug, PartialEq, Eq, YamlRt)]
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

#[derive(Debug, PartialEq, Eq, YamlRt)]
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

#[derive(Debug, PartialEq, Eq, YamlRt)]
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
