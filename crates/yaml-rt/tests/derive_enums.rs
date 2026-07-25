use std::time::Duration;

use yaml_rt::{FromYamlDoc, ToYamlDoc, ToYamlFragment, YamlDoc, YamlRoundTrip};

#[derive(Debug, PartialEq, Eq, YamlRoundTrip)]
#[yaml(rename_all = "lowercase")]
enum Letter {
    A,
    B,
    C,
}

#[derive(Debug, PartialEq, Eq, YamlRoundTrip)]
enum Status {
    Pending,
    #[yaml(rename = "ready", alias = "legacy-ready")]
    Ready,
}

#[derive(Debug, PartialEq, Eq, YamlRoundTrip)]
struct UnitEnumConfig {
    letter: Letter,
    status: Status,
}

mod duration_seconds {
    use std::time::Duration;

    use yaml_rt::YamlError;

    pub type Repr = u64;

    pub fn from_yaml(value: Repr) -> Result<Duration, YamlError> {
        Ok(Duration::from_secs(value))
    }

    pub fn to_yaml(value: &Duration) -> Result<Repr, YamlError> {
        Ok(value.as_secs())
    }
}

#[derive(Debug, PartialEq, Eq, YamlRoundTrip)]
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
    #[derive(YamlRoundTrip)]
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
        Mode::Delayed(Duration::from_secs(60))
            .to_yaml_fragment(0, "\n")
            .expect("adapted variant formats"),
        "!delayed 60"
    );
}

#[test]
fn variant_switch_replaces_payload_but_preserves_anchor_and_entry_comment() {
    #[derive(Debug, PartialEq, Eq, YamlRoundTrip)]
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
