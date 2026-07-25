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
