use std::time::Duration;

use yaml_rt::{FromYamlDoc, ToYamlDoc, ToYamlFragment, YamlDoc, YamlRt};

#[derive(Debug, PartialEq, Eq, YamlRt)]
struct Disabled;

#[derive(Debug, PartialEq, Eq, YamlRt)]
struct Endpoint(String, u16);

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

#[derive(Debug, PartialEq, Eq, YamlRt)]
struct Window<T, const N: usize>(T, [u8; N], #[yaml(with = "duration_seconds")] Duration)
where
    T: Copy;

#[derive(Debug, PartialEq, Eq, YamlRt)]
struct Config {
    disabled: Disabled,
    endpoint: Endpoint,
}

#[test]
fn unit_struct_accepts_and_preserves_core_null_spellings() {
    for yaml in ["null\n", "~\n", "Null\n", "NULL\n", "!!null Null\n"] {
        let mut doc = YamlDoc::parse(yaml).expect("valid null document");
        let value = Disabled::from_yaml_doc(&doc).expect("unit struct reads null");
        value
            .apply_to_yaml_doc(&mut doc)
            .expect("unchanged unit struct writes");
        assert_eq!(doc.to_string(), yaml);
    }
}

#[test]
fn unit_struct_replaces_non_null_nodes_and_reports_invalid_reads() {
    let doc = YamlDoc::parse("enabled\n").expect("valid scalar document");
    let error = Disabled::from_yaml_doc(&doc).expect_err("non-null unit struct must fail");
    assert!(error.diagnostic.message.contains("expected YAML null"));
    assert!(error.diagnostic.position.is_some());

    let mut nested = YamlDoc::parse("disabled: false # retain\nendpoint: [api, 80]\n")
        .expect("valid config document");
    Config {
        disabled: Disabled,
        endpoint: Endpoint("api".to_owned(), 80),
    }
    .apply_to_yaml_doc(&mut nested)
    .expect("unit struct replaces nested value");
    assert_eq!(
        nested.to_string(),
        "disabled: null # retain\nendpoint: [api, 80]\n"
    );
    assert_eq!(Disabled.to_yaml_fragment(0, "\n").unwrap(), "null");
}

#[test]
fn selected_documents_support_unit_struct_roots() {
    let mut doc = YamlDoc::parse("name: app\n---\n~\n").expect("valid stream");
    let value: Disabled = doc.read_document(1).expect("selected unit struct reads");
    doc.write_document(1, &value)
        .expect("selected unit struct writes");
    assert_eq!(doc.to_string(), "name: app\n---\n~\n");
}

#[test]
fn selected_documents_support_tuple_struct_roots() {
    let mut doc = YamlDoc::parse("name: app\n---\n[api, 80]\n").expect("valid stream");
    let mut endpoint: Endpoint = doc.read_document(1).expect("selected tuple struct reads");
    endpoint.1 = 443;
    doc.write_document(1, &endpoint)
        .expect("selected tuple struct writes");
    assert_eq!(doc.to_string(), "name: app\n---\n[api, 443]\n");
}

#[test]
fn tuple_struct_patches_flow_and_block_sequences_losslessly() {
    let mut flow = YamlDoc::parse("[api, 0x50] # endpoint\n").expect("valid flow tuple");
    let mut endpoint = Endpoint::from_yaml_doc(&flow).expect("flow tuple reads");
    endpoint.1 = 443;
    endpoint
        .apply_to_yaml_doc(&mut flow)
        .expect("flow tuple writes");
    assert_eq!(flow.to_string(), "[api, 443] # endpoint\n");

    let mut block = YamlDoc::parse("- api # host\n- 0x50 # port\n").expect("valid block tuple");
    endpoint = Endpoint::from_yaml_doc(&block).expect("block tuple reads");
    endpoint.0 = "edge".to_owned();
    endpoint
        .apply_to_yaml_doc(&mut block)
        .expect("block tuple writes");
    assert_eq!(block.to_string(), "- edge # host\n- 0x50 # port\n");
}

#[test]
fn tuple_structs_work_as_nested_values_and_fragments() {
    let mut doc =
        YamlDoc::parse("disabled: ~\nendpoint: [api, 80]\n").expect("valid nested tuple document");
    let mut config = Config::from_yaml_doc(&doc).expect("nested tuple reads");
    config.endpoint.1 = 8080;
    config
        .apply_to_yaml_doc(&mut doc)
        .expect("nested tuple writes");
    assert_eq!(doc.to_string(), "disabled: ~\nendpoint: [api, 8080]\n");
    assert_eq!(
        Endpoint("api".to_owned(), 80)
            .to_yaml_fragment(0, "\n")
            .unwrap(),
        "- api\n- 80"
    );
}

#[test]
fn tuple_structs_validate_kind_and_exact_arity() {
    for (yaml, message) in [
        ("host: api\n", "requires a sequence"),
        ("[api]\n", "expects 2 fields, found 1"),
        ("[api, 80, extra]\n", "expects 2 fields, found 3"),
    ] {
        let doc = YamlDoc::parse(yaml).expect("valid YAML");
        let error = Endpoint::from_yaml_doc(&doc).expect_err("tuple shape must fail");
        assert!(error.diagnostic.message.contains(message));
        assert!(error.diagnostic.position.is_some());
    }

    let mut wrong_arity = YamlDoc::parse("[api]\n").expect("valid short sequence");
    let error = Endpoint("edge".to_owned(), 443)
        .apply_to_yaml_doc(&mut wrong_arity)
        .expect_err("tuple write must reject the wrong arity");
    assert!(
        error
            .diagnostic
            .message
            .contains("expects 2 fields, found 1")
    );
    assert_eq!(wrong_arity.to_string(), "[api]\n");
}

#[test]
fn tuple_structs_support_generics_const_generics_and_adapters() {
    let mut doc = YamlDoc::parse("[0x10, [1, 2], 30]\n").expect("valid generic tuple");
    let mut window: Window<u16, 2> = Window::from_yaml_doc(&doc).expect("generic tuple reads");
    assert_eq!(window, Window(16, [1, 2], Duration::from_secs(30)));
    window.0 = 17;
    window.2 = Duration::from_secs(45);
    window
        .apply_to_yaml_doc(&mut doc)
        .expect("generic tuple writes");
    assert_eq!(doc.to_string(), "[17, [1, 2], 45]\n");
}
