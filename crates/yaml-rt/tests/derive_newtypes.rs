use std::time::Duration;

use yaml_rt::{FromYamlDoc, ToYamlDoc, YamlDoc, YamlRt};

#[derive(Debug, PartialEq, Eq, YamlRt)]
struct Port(u16);

#[derive(Debug, PartialEq, Eq, YamlRt)]
struct Identifier<T>(T)
where
    T: Copy;

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
struct Timeout(#[yaml(with = "duration_seconds")] Duration);

#[derive(Debug, PartialEq, Eq, YamlRt)]
struct Service {
    port: Port,
    identifiers: Vec<Identifier<u16>>,
    timeout: Timeout,
}

#[test]
fn transparent_newtype_reads_and_writes_scalar_document_losslessly() {
    let mut doc = YamlDoc::parse("0x10\n").expect("valid scalar document");
    let mut port = Port::from_yaml_doc(&doc).expect("newtype reads root scalar");

    assert_eq!(port, Port(16));
    port.apply_to_yaml_doc(&mut doc)
        .expect("unchanged newtype writes");
    assert_eq!(doc.to_string(), "0x10\n");

    port.0 = 17;
    port.apply_to_yaml_doc(&mut doc)
        .expect("changed newtype writes");
    assert_eq!(doc.to_string(), "17\n");
}

#[test]
fn transparent_newtypes_work_in_fields_collections_and_with_adapters() {
    let mut doc = YamlDoc::parse("port: 8080\nidentifiers: [1, 2]\ntimeout: 30\nextra: keep\n")
        .expect("valid newtype config");
    let mut service = Service::from_yaml_doc(&doc).expect("nested newtypes read");

    assert_eq!(service.port, Port(8080));
    assert_eq!(service.identifiers, vec![Identifier(1), Identifier(2)]);
    assert_eq!(service.timeout, Timeout(Duration::from_secs(30)));

    service.port.0 = 9090;
    service.identifiers.push(Identifier(3));
    service.timeout.0 = Duration::from_secs(45);
    service
        .apply_to_yaml_doc(&mut doc)
        .expect("nested newtypes write");

    assert_eq!(
        doc.to_string(),
        "port: 9090\nidentifiers: [1, 2, 3]\ntimeout: 45\nextra: keep\n"
    );
}

#[test]
fn selected_documents_support_transparent_newtype_roots() {
    let mut doc = YamlDoc::parse("name: app\n---\n42\n").expect("valid stream");
    let mut identifier: Identifier<u16> = doc.read_document(1).expect("selected newtype reads");
    identifier.0 = 43;
    doc.write_document(1, &identifier)
        .expect("selected newtype writes");

    assert_eq!(doc.to_string(), "name: app\n---\n43\n");
}
