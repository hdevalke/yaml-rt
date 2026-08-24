use std::{
    collections::{BTreeMap, HashMap},
    net::Ipv4Addr,
    time::Duration,
};

use yaml_rt::{FromYamlDoc, ToYamlDoc, ToYamlFragment, YamlDoc, YamlRt};

#[derive(Debug, PartialEq, Eq, YamlRt)]
#[yaml(rename_all = "lowercase")]
enum Letter {
    A,
    B,
    C,
}

#[derive(Debug, PartialEq, Eq, YamlRt)]
enum Status {
    Pending,
    #[yaml(rename = "ready", alias = "legacy-ready")]
    Ready,
}

#[derive(Debug, PartialEq, Eq, YamlRt)]
struct UnitEnumConfig {
    letter: Letter,
    status: Status,
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

mod ipv4_octets {
    use std::net::Ipv4Addr;

    use yaml_rt::YamlError;

    pub type Repr = [u8; 4];

    #[expect(
        clippy::unnecessary_wraps,
        reason = "yaml(with) adapters require fallible conversion signatures"
    )]
    pub fn from_yaml(value: Repr) -> Result<Ipv4Addr, YamlError> {
        Ok(Ipv4Addr::from(value))
    }

    #[expect(
        clippy::trivially_copy_pass_by_ref,
        reason = "yaml(with) adapter write callbacks receive a reference to the field"
    )]
    #[expect(
        clippy::unnecessary_wraps,
        reason = "yaml(with) adapters require fallible conversion signatures"
    )]
    pub fn to_yaml(value: &Ipv4Addr) -> Result<Repr, YamlError> {
        Ok(value.octets())
    }
}

mod positive_i16 {
    use yaml_rt::{Diagnostic, DiagnosticKind, Span, YamlError};

    pub type Repr = i16;

    pub fn from_yaml(value: Repr) -> Result<i16, YamlError> {
        if value >= 0 {
            Ok(value)
        } else {
            Err(conversion_error("negative YAML value"))
        }
    }

    #[expect(
        clippy::trivially_copy_pass_by_ref,
        reason = "yaml(with) adapter write callbacks receive a reference to the field"
    )]
    pub fn to_yaml(value: &i16) -> Result<Repr, YamlError> {
        if *value >= 0 {
            Ok(*value)
        } else {
            Err(conversion_error("negative Rust value"))
        }
    }

    fn conversion_error(message: &str) -> YamlError {
        YamlError::new(Diagnostic::new(
            DiagnosticKind::Typed,
            message,
            Span::empty(0),
        ))
    }
}

#[derive(Debug, PartialEq, Eq, YamlRt)]
#[yaml(rename_all = "lowercase")]
enum Mode {
    A,
    #[yaml(alias = "legacy-value")]
    Value(u16),
    Pair(u8, bool),
    Server {
        host: String,
        #[yaml(default = 8080)]
        port: u16,
    },
    Delayed(#[yaml(with = "duration_seconds")] Duration),
}

#[derive(Debug, PartialEq, Eq, YamlRt)]
enum FieldRenamedMode {
    #[yaml(rename_all = "camelCase")]
    ServerConfig { host_name: String, max_retries: u8 },
}

#[test]
fn named_enum_variant_rename_all_patches_block_and_inserts_flow_fields() {
    let mut block = YamlDoc::parse("!ServerConfig\nhostName: old\nmaxRetries: 1\n").unwrap();
    let mut value = FieldRenamedMode::from_yaml_doc(&block).unwrap();
    value = match value {
        FieldRenamedMode::ServerConfig { .. } => FieldRenamedMode::ServerConfig {
            host_name: "new".to_owned(),
            max_retries: 2,
        },
    };
    value.apply_to_yaml_doc(&mut block).unwrap();
    assert_eq!(
        block.to_string(),
        "!ServerConfig\nhostName: new\nmaxRetries: 2\n"
    );

    let mut flow = YamlDoc::parse("!ServerConfig {hostName: old}\n").unwrap();
    value.apply_to_yaml_doc(&mut flow).unwrap();
    assert_eq!(
        flow.to_string(),
        "!ServerConfig {hostName: new, maxRetries: 2}\n"
    );
}

#[derive(Debug, PartialEq, Eq, YamlRt)]
#[yaml(rename_all = "lowercase")]
enum AdaptedPayload {
    Network(#[yaml(with = "ipv4_octets")] Ipv4Addr),
    Positive(#[yaml(with = "positive_i16")] i16),
}

#[derive(Debug, PartialEq, Eq, YamlRt)]
#[yaml(rename_all = "lowercase")]
enum EmptyPayload {
    Tuple(),
    Struct {},
}

#[derive(Debug, PartialEq, Eq, YamlRt)]
#[yaml(rename_all = "lowercase")]
enum ExtensibleMode {
    Config {
        name: String,
        #[yaml(flatten)]
        extra: BTreeMap<String, String>,
    },
}

#[derive(Debug, PartialEq, Eq, YamlRt)]
enum GenericMode<T, const N: usize>
where
    T: Copy,
{
    Value(T),
    Batch([T; N]),
}

#[derive(Debug, PartialEq, Eq, YamlRt)]
struct EnumShapes {
    optional: Option<Mode>,
    sequence: Vec<Mode>,
    array: [Mode; 2],
    boxed: Box<Mode>,
    ordered: BTreeMap<String, Mode>,
    hashed: HashMap<String, Mode>,
}

#[test]
fn lowercase_unit_enum_matches_direct_scalars_and_patches_losslessly() {
    let mut doc = YamlDoc::parse("a # selected\n").expect("valid unit enum scalar");
    let mut value = Letter::from_yaml_doc(&doc).expect("unit enum reads");
    assert_eq!(value, Letter::A);

    value
        .apply_to_yaml_doc(&mut doc)
        .expect("unchanged unit enum writes");
    assert_eq!(doc.to_string(), "a # selected\n");

    value = Letter::B;
    value
        .apply_to_yaml_doc(&mut doc)
        .expect("changed unit enum writes");
    assert_eq!(doc.to_string(), "b # selected\n");
}

#[test]
fn unit_variant_alias_and_quoting_survive_unchanged_writes() {
    let mut doc = YamlDoc::parse("\"legacy-ready\"\n").expect("valid aliased unit enum");
    let value = Status::from_yaml_doc(&doc).expect("variant alias reads");
    assert_eq!(value, Status::Ready);

    value
        .apply_to_yaml_doc(&mut doc)
        .expect("unchanged alias writes");
    assert_eq!(doc.to_string(), "\"legacy-ready\"\n");
}

#[test]
fn unit_enums_work_as_named_struct_fields() {
    let mut doc =
        YamlDoc::parse("letter: c\nstatus: Pending\nextra: keep\n").expect("valid enum config");
    let mut config = UnitEnumConfig::from_yaml_doc(&doc).expect("enum fields read");

    config.letter = Letter::A;
    config.status = Status::Ready;
    config
        .apply_to_yaml_doc(&mut doc)
        .expect("enum fields write");

    assert_eq!(doc.to_string(), "letter: a\nstatus: ready\nextra: keep\n");
}

#[test]
fn unit_enum_fragments_quote_schema_ambiguous_variant_names() {
    #[derive(YamlRt)]
    enum Ambiguous {
        #[yaml(rename = "true")]
        Enabled,
    }

    assert_eq!(
        Ambiguous::Enabled
            .to_yaml_fragment(0, "\n")
            .expect("variant formats"),
        "\"true\""
    );
}

#[test]
fn unknown_unit_variant_reports_a_positioned_error() {
    let doc = YamlDoc::parse("unknown\n").expect("valid YAML scalar");
    let error = Letter::from_yaml_doc(&doc).expect_err("unknown variant must fail");

    assert!(
        error
            .diagnostic
            .message
            .contains("unknown YAML enum variant")
    );
    assert!(error.diagnostic.position.is_some());
    assert_eq!(error.diagnostic.expected, ["a", "b", "c"]);
}

#[test]
fn tagged_newtype_variant_patches_payload_and_preserves_alias_tag() {
    let mut doc = YamlDoc::parse("!legacy-value 0x10 # chosen\n").expect("valid tagged scalar");
    let mut mode = Mode::from_yaml_doc(&doc).expect("tagged newtype reads");
    assert_eq!(mode, Mode::Value(16));

    mode.apply_to_yaml_doc(&mut doc)
        .expect("unchanged tagged newtype writes");
    assert_eq!(doc.to_string(), "!legacy-value 0x10 # chosen\n");

    mode = Mode::Value(17);
    mode.apply_to_yaml_doc(&mut doc)
        .expect("same tagged variant writes incrementally");
    assert_eq!(doc.to_string(), "!legacy-value 17 # chosen\n");
}

#[test]
fn tagged_tuple_variant_preserves_flow_and_block_payload_presentation() {
    let mut flow = YamlDoc::parse("!pair [1, FALSE] # tuple\n").expect("valid flow tuple");
    let mut mode = Mode::from_yaml_doc(&flow).expect("flow tuple reads");
    assert_eq!(mode, Mode::Pair(1, false));

    mode = Mode::Pair(2, true);
    mode.apply_to_yaml_doc(&mut flow)
        .expect("flow tuple patches");
    assert_eq!(flow.to_string(), "!pair [2, true] # tuple\n");

    let mut block =
        YamlDoc::parse("!pair\n- 1 # first\n- false # second\n").expect("valid block tuple");
    mode = Mode::from_yaml_doc(&block).expect("block tuple reads");
    assert_eq!(mode, Mode::Pair(1, false));

    mode = Mode::Pair(3, true);
    mode.apply_to_yaml_doc(&mut block)
        .expect("block tuple patches");
    assert_eq!(block.to_string(), "!pair\n- 3 # first\n- true # second\n");
}

#[test]
fn tagged_struct_variant_preserves_unknown_fields_and_comments() {
    let mut doc = YamlDoc::parse("!server\nhost: api # endpoint\nunknown: keep # extension\n")
        .expect("valid tagged mapping");
    let mut mode = Mode::from_yaml_doc(&doc).expect("struct variant reads");
    assert_eq!(
        mode,
        Mode::Server {
            host: "api".to_owned(),
            port: 8080,
        }
    );

    mode = Mode::Server {
        host: "web".to_owned(),
        port: 9090,
    };
    mode.apply_to_yaml_doc(&mut doc)
        .expect("struct variant patches");
    assert_eq!(
        doc.to_string(),
        "!server\nhost: web # endpoint\nunknown: keep # extension\nport: 9090\n"
    );
}

#[test]
fn tagged_payload_adapters_read_write_and_format() {
    let mut doc = YamlDoc::parse("!delayed 30\n").expect("valid adapted variant");
    let mut mode = Mode::from_yaml_doc(&doc).expect("adapted variant reads");
    assert_eq!(mode, Mode::Delayed(Duration::from_secs(30)));

    mode = Mode::Delayed(Duration::from_secs(45));
    mode.apply_to_yaml_doc(&mut doc)
        .expect("adapted variant writes");
    assert_eq!(doc.to_string(), "!delayed 45\n");

    assert_eq!(
        Mode::Delayed(Duration::from_mins(1))
            .to_yaml_fragment(0, "\n")
            .expect("adapted variant formats"),
        "!delayed 60"
    );
}

#[test]
fn tagged_payload_adapters_support_collections_and_propagate_errors() {
    let mut doc =
        YamlDoc::parse("!network [127, 0, 0, 1]\n").expect("valid collection adapter payload");
    let mut value = AdaptedPayload::from_yaml_doc(&doc).expect("collection adapter reads");
    assert_eq!(value, AdaptedPayload::Network(Ipv4Addr::LOCALHOST));

    value = AdaptedPayload::Network(Ipv4Addr::new(10, 0, 0, 1));
    value
        .apply_to_yaml_doc(&mut doc)
        .expect("collection adapter writes");
    assert_eq!(doc.to_string(), "!network [10, 0, 0, 1]\n");

    let invalid = YamlDoc::parse("!positive -1\n").expect("valid negative payload");
    let read_error =
        AdaptedPayload::from_yaml_doc(&invalid).expect_err("adapter read error propagates");
    assert!(
        read_error
            .diagnostic
            .message
            .contains("negative YAML value")
    );

    value = AdaptedPayload::Positive(-1);
    let write_error = value
        .apply_to_yaml_doc(&mut YamlDoc::parse("!positive 1\n").unwrap())
        .expect_err("adapter write error propagates");
    assert!(
        write_error
            .diagnostic
            .message
            .contains("negative Rust value")
    );
}

#[test]
fn variant_switch_replaces_payload_but_preserves_anchor_and_entry_comment() {
    #[derive(Debug, PartialEq, Eq, YamlRt)]
    struct Config {
        mode: Mode,
    }

    let mut doc = YamlDoc::parse("mode: &selected !value 1 # keep entry\nother: keep\n")
        .expect("valid anchored enum field");
    let mut config = Config::from_yaml_doc(&doc).expect("enum field reads");
    config.mode = Mode::Pair(2, true);
    config
        .apply_to_yaml_doc(&mut doc)
        .expect("variant switch writes");
    doc.commit_edits().expect("variant switch reparses");

    assert_eq!(
        doc.to_string(),
        "mode: !pair &selected\n - 2\n - true # keep entry\nother: keep\n"
    );
    assert_eq!(
        Config::from_yaml_doc(&doc)
            .expect("switched enum rereads")
            .mode,
        Mode::Pair(2, true)
    );
}

#[test]
fn switching_from_data_to_unit_removes_the_old_tag() {
    let mut doc = YamlDoc::parse("&mode !value 1 # keep\n").expect("valid tagged enum");
    let mode = Mode::A;

    mode.apply_to_yaml_doc(&mut doc)
        .expect("data-to-unit switch writes");
    doc.commit_edits().expect("data-to-unit switch reparses");

    assert_eq!(doc.to_string(), "&mode a # keep\n");
    assert_eq!(Mode::from_yaml_doc(&doc).expect("unit rereads"), Mode::A);
    let root = doc.document_root(0).unwrap().unwrap();
    assert_eq!(doc.raw_tag(root), None);
    assert_eq!(doc.anchor(root), Some("mode"));
}

#[test]
fn tagged_variant_fragments_use_serde_compatible_local_tags() {
    assert_eq!(
        Mode::Value(42)
            .to_yaml_fragment(0, "\n")
            .expect("newtype variant formats"),
        "!value 42"
    );
    assert_eq!(
        Mode::Pair(1, true)
            .to_yaml_fragment(0, "\n")
            .expect("tuple variant formats"),
        "!pair\n- 1\n- true"
    );
    assert_eq!(
        Mode::Server {
            host: "api".to_owned(),
            port: 8080,
        }
        .to_yaml_fragment(0, "\n")
        .expect("struct variant formats"),
        "!server\nhost: api\nport: 8080"
    );
}

#[test]
fn struct_variants_support_catch_all_maps_in_edits_and_fragments() {
    let mut doc = YamlDoc::parse("!config {name: app, custom: \"old\", remove: gone}\n")
        .expect("valid extensible enum variant");
    let mut value = ExtensibleMode::from_yaml_doc(&doc).expect("enum catch-all reads");
    let ExtensibleMode::Config { name, extra } = &mut value;
    assert_eq!(name, "app");
    extra.insert("custom".to_owned(), "new".to_owned());
    extra.remove("remove");
    extra.insert("alpha".to_owned(), "first".to_owned());

    value
        .apply_to_yaml_doc(&mut doc)
        .expect("enum catch-all writes");
    assert_eq!(
        doc.to_string(),
        "!config {name: app, custom: \"new\", alpha: first}\n"
    );
    doc.commit_edits().expect("enum catch-all output reparses");
    assert_eq!(
        ExtensibleMode::from_yaml_doc(&doc).expect("enum catch-all rereads"),
        value
    );

    assert_eq!(
        value
            .to_yaml_fragment(0, "\n")
            .expect("enum catch-all fragment formats"),
        "!config\nname: app\nalpha: first\ncustom: new"
    );
}

#[test]
fn enums_work_in_common_configuration_containers() {
    let yaml = "\
optional: !value 1
sequence:
  - !pair [2, false] # positional
  - a
array: [!value 3, !server {host: array, port: 8000}]
boxed: !delayed 30
ordered:
  first: !value 4
hashed: {one: !pair [5, true]}
";
    let mut doc = YamlDoc::parse(yaml).expect("valid nested enum shapes");
    let mut shapes = EnumShapes::from_yaml_doc(&doc).expect("nested enums read");

    shapes
        .apply_to_yaml_doc(&mut doc)
        .expect("unchanged nested enums write");
    assert_eq!(doc.to_string(), yaml);

    shapes.optional = Some(Mode::Pair(10, true));
    shapes.sequence[0] = Mode::Pair(20, true);
    shapes.array[1] = Mode::Server {
        host: "updated".to_owned(),
        port: 8001,
    };
    *shapes.boxed = Mode::Delayed(Duration::from_secs(45));
    shapes.ordered.insert("first".to_owned(), Mode::Value(40));
    shapes
        .hashed
        .insert("one".to_owned(), Mode::Pair(50, false));
    shapes
        .apply_to_yaml_doc(&mut doc)
        .expect("nested enums patch");
    doc.commit_edits().expect("nested enum edits reparse");

    let reread = EnumShapes::from_yaml_doc(&doc).expect("nested enums reread");
    assert_eq!(reread, shapes);
    assert!(doc.to_string().contains("# positional"));
    assert!(doc.to_string().contains("array: ["));
    assert!(doc.to_string().contains("hashed: {"));
}

#[test]
fn generic_enums_support_scalar_and_array_payloads_at_roots() {
    let mut scalar = YamlDoc::parse("!Value 0x10\n").expect("valid generic scalar payload");
    let mut value =
        GenericMode::<u16, 2>::from_yaml_doc(&scalar).expect("generic enum scalar reads");
    assert_eq!(value, GenericMode::Value(16));
    value = GenericMode::Value(17);
    value
        .apply_to_yaml_doc(&mut scalar)
        .expect("generic enum scalar writes");
    assert_eq!(scalar.to_string(), "!Value 17\n");

    let mut array = YamlDoc::parse("!Batch [1, 2]\n").expect("valid generic array payload");
    value = GenericMode::<u16, 2>::from_yaml_doc(&array).expect("generic enum array reads");
    assert_eq!(value, GenericMode::Batch([1, 2]));
    value = GenericMode::Batch([3, 4]);
    value
        .apply_to_yaml_doc(&mut array)
        .expect("generic enum array writes");
    assert_eq!(array.to_string(), "!Batch [3, 4]\n");
}

#[test]
fn selected_documents_support_tagged_enum_roots() {
    let mut doc = YamlDoc::parse("name: app\n---\n!value 1\n").expect("valid stream");
    assert_eq!(
        doc.read_document::<Mode>(1).expect("selected enum reads"),
        Mode::Value(1)
    );
    let mode = Mode::Server {
        host: "api".to_owned(),
        port: 8080,
    };
    doc.write_document(1, &mode).expect("selected enum writes");
    doc.commit_edits().expect("selected enum edit reparses");

    assert_eq!(
        doc.read_document::<Mode>(1).expect("selected enum rereads"),
        mode
    );
    assert_eq!(
        doc.to_string(),
        "name: app\n---\n!server\nhost: api\nport: 8080\n"
    );
}

#[test]
fn tagged_enum_failures_are_positioned_and_describe_the_payload_contract() {
    let cases = [
        ("!unknown 1\n", "unknown YAML enum tag"),
        ("[1, true]\n", "requires a local YAML tag"),
        ("!pair 1\n", "requires a sequence payload"),
        ("!pair [1]\n", "expects 2 tuple fields"),
        ("!server [1]\n", "expected mapping"),
    ];

    for (yaml, message) in cases {
        let doc = YamlDoc::parse(yaml).expect("failure case is valid YAML");
        let error = Mode::from_yaml_doc(&doc).expect_err("typed enum read must fail");
        assert!(
            error.diagnostic.message.contains(message),
            "{yaml:?} produced {:?}",
            error.diagnostic.message
        );
        assert!(error.diagnostic.position.is_some(), "{yaml:?}");
    }
}

#[test]
fn data_only_enums_report_a_missing_tag_for_untagged_scalars() {
    let doc = YamlDoc::parse("127\n").expect("valid untagged scalar");
    let error =
        AdaptedPayload::from_yaml_doc(&doc).expect_err("data enum requires a local YAML tag");

    assert!(
        error
            .diagnostic
            .message
            .contains("requires a local YAML tag")
    );
    assert!(error.diagnostic.position.is_some());
}

#[test]
fn empty_tuple_and_struct_variant_payloads_use_explicit_empty_collections() {
    assert_eq!(
        EmptyPayload::Tuple().to_yaml_fragment(0, "\n").unwrap(),
        "!tuple []"
    );
    assert_eq!(
        EmptyPayload::Struct {}.to_yaml_fragment(0, "\n").unwrap(),
        "!struct {}"
    );

    let tuple = YamlDoc::parse("!tuple []\n").expect("valid empty tuple payload");
    let structure = YamlDoc::parse("!struct {}\n").expect("valid empty struct payload");
    assert_eq!(
        EmptyPayload::from_yaml_doc(&tuple).expect("empty tuple reads"),
        EmptyPayload::Tuple()
    );
    assert_eq!(
        EmptyPayload::from_yaml_doc(&structure).expect("empty struct reads"),
        EmptyPayload::Struct {}
    );
}

#[test]
fn enum_rename_rules_follow_serde_variant_transformations() {
    #[derive(YamlRt)]
    #[yaml(rename_all = "lowercase")]
    enum Lower {
        VeryTasty(u8),
    }
    #[derive(YamlRt)]
    #[yaml(rename_all = "snake_case")]
    enum Snake {
        VeryTasty(u8),
    }
    #[derive(YamlRt)]
    #[yaml(rename_all = "kebab-case")]
    enum Kebab {
        VeryTasty(u8),
    }
    #[derive(YamlRt)]
    #[yaml(rename_all = "SCREAMING_SNAKE_CASE")]
    enum Screaming {
        VeryTasty(u8),
    }
    #[derive(YamlRt)]
    #[yaml(rename_all = "camelCase")]
    enum Camel {
        VeryTasty(u8),
    }
    #[derive(YamlRt)]
    #[yaml(rename_all = "PascalCase")]
    enum Pascal {
        VeryTasty(u8),
    }

    assert_eq!(
        Lower::VeryTasty(1).to_yaml_fragment(0, "\n").unwrap(),
        "!verytasty 1"
    );
    assert_eq!(
        Snake::VeryTasty(1).to_yaml_fragment(0, "\n").unwrap(),
        "!very_tasty 1"
    );
    assert_eq!(
        Kebab::VeryTasty(1).to_yaml_fragment(0, "\n").unwrap(),
        "!very-tasty 1"
    );
    assert_eq!(
        Screaming::VeryTasty(1).to_yaml_fragment(0, "\n").unwrap(),
        "!VERY_TASTY 1"
    );
    assert_eq!(
        Camel::VeryTasty(1).to_yaml_fragment(0, "\n").unwrap(),
        "!veryTasty 1"
    );
    assert_eq!(
        Pascal::VeryTasty(1).to_yaml_fragment(0, "\n").unwrap(),
        "!VeryTasty 1"
    );
}

#[test]
fn optional_enums_distinguish_missing_null_and_tagged_values() {
    #[derive(Debug, PartialEq, Eq, YamlRt)]
    struct OptionalMode {
        mode: Option<Mode>,
    }

    let missing = YamlDoc::parse("other: keep\n").expect("valid missing option");
    assert_eq!(
        OptionalMode::from_yaml_doc(&missing)
            .expect("missing enum option reads")
            .mode,
        None
    );

    let mut doc = YamlDoc::parse("mode: null # optional\n").expect("valid null option");
    let mut config = OptionalMode::from_yaml_doc(&doc).expect("null enum option reads");
    assert_eq!(config.mode, None);
    config.mode = Some(Mode::Value(7));
    config
        .apply_to_yaml_doc(&mut doc)
        .expect("present enum option writes");
    doc.commit_edits().expect("present enum option reparses");
    assert_eq!(doc.to_string(), "mode: !value 7 # optional\n");

    config.mode = None;
    config
        .apply_to_yaml_doc(&mut doc)
        .expect("null enum option writes");
    doc.commit_edits().expect("null enum option reparses");
    assert_eq!(doc.to_string(), "mode: null # optional\n");
    assert_eq!(
        OptionalMode::from_yaml_doc(&doc)
            .expect("rewritten null option reads")
            .mode,
        None
    );
}

#[test]
fn enum_sequence_growth_renders_tags_in_flow_context_and_preserves_crlf() {
    #[derive(Debug, PartialEq, Eq, YamlRt)]
    struct Modes {
        modes: Vec<Mode>,
    }

    let mut doc = YamlDoc::parse("modes: [a]\r\n").expect("valid CRLF flow enum sequence");
    let mut config = Modes::from_yaml_doc(&doc).expect("enum sequence reads");
    config.modes.push(Mode::Pair(1, true));
    config
        .apply_to_yaml_doc(&mut doc)
        .expect("enum sequence grows");
    doc.commit_edits().expect("enum sequence growth reparses");

    assert_eq!(doc.to_string(), "modes: [a, !pair [1, true]]\r\n");
    assert_eq!(
        Modes::from_yaml_doc(&doc).expect("grown enum sequence rereads"),
        config
    );
}
