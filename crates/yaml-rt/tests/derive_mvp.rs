use yaml_rt::{FromYamlDoc, ToYamlDoc, YamlDoc, YamlRt};

#[derive(Debug, PartialEq, Eq, YamlRt)]
struct Config {
    host: String,
    port: u16,
}

#[derive(Debug, PartialEq, Eq, YamlRt)]
struct MatrixConfig {
    host: String,
    matrix: Vec<Vec<u16>>,
}

#[derive(Debug, PartialEq, Eq, YamlRt)]
struct PortsConfig {
    host: String,
    ports: Vec<u16>,
}

#[test]
fn derive_reads_and_updates_root_mapping_fields() {
    let mut doc =
        YamlDoc::parse("host: \"localhost\"\nport: 3000\nextra: keep\n").expect("valid MVP YAML");
    let mut config = Config::from_yaml_doc(&doc).expect("derive reads config");

    assert_eq!(
        config,
        Config {
            host: "localhost".to_owned(),
            port: 3000,
        }
    );

    config.port = 9090;
    config
        .apply_to_yaml_doc(&mut doc)
        .expect("derive writes config");

    assert_eq!(
        doc.to_string(),
        "host: \"localhost\"\nport: 9090\nextra: keep\n"
    );
}

#[test]
fn derive_inserts_missing_fields_at_the_end() {
    let mut doc = YamlDoc::parse("host: localhost\n").expect("valid MVP YAML");
    let config = Config {
        host: "localhost".to_owned(),
        port: 8080,
    };

    config
        .apply_to_yaml_doc(&mut doc)
        .expect("derive inserts missing port");

    assert_eq!(doc.to_string(), "host: localhost\nport: 8080\n");
}

#[test]
fn derive_reads_and_writes_selected_document() {
    let mut doc = YamlDoc::parse(
        "---\nhost: first\nport: 1000\n---\n# selected\nhost: \"second\"\nport: 2000\nextra: keep\n",
    )
    .expect("valid multi-document stream");
    let mut config: Config = doc.read_document(1).expect("derive reads second document");

    assert_eq!(
        config,
        Config {
            host: "second".to_owned(),
            port: 2000,
        }
    );

    config.port = 9090;
    doc.write_document(1, &config)
        .expect("derive writes second document");

    assert_eq!(
        doc.to_string(),
        "---\nhost: first\nport: 1000\n---\n# selected\nhost: \"second\"\nport: 9090\nextra: keep\n"
    );
}

#[test]
fn derive_inserts_and_rewrites_nested_collection_field() {
    let mut doc = YamlDoc::parse("host: localhost\n").expect("valid YAML");
    let config = MatrixConfig {
        host: "localhost".to_owned(),
        matrix: vec![vec![1, 2], vec![3]],
    };

    config
        .apply_to_yaml_doc(&mut doc)
        .expect("derive inserts nested collection");

    assert_eq!(
        doc.to_string(),
        "host: localhost\nmatrix:\n  -\n    - 1\n    - 2\n  -\n    - 3\n"
    );

    doc.commit_edits().expect("inserted matrix commits");
    let mut updated = MatrixConfig::from_yaml_doc(&doc).expect("derive reads inserted matrix");
    updated.matrix = vec![vec![4], vec![5, 6], vec![7]];
    updated
        .apply_to_yaml_doc(&mut doc)
        .expect("derive rewrites nested collection");

    assert_eq!(
        doc.to_string(),
        "host: localhost\nmatrix:\n  -\n    - 4\n  -\n    - 5\n    - 6\n  -\n    - 7\n"
    );
}

#[test]
fn derive_resizes_block_sequence_field_with_minimal_diff() {
    let mut doc = YamlDoc::parse(
        "host: localhost\nports:\n  - 8080 # first\n  - 9090 # second\nextra: keep\n",
    )
    .expect("valid YAML");
    let mut config = PortsConfig::from_yaml_doc(&doc).expect("derive reads ports");

    config.ports = vec![3000, 3001, 3002];
    config
        .apply_to_yaml_doc(&mut doc)
        .expect("derive grows ports sequence");

    assert_eq!(
        doc.to_string(),
        "host: localhost\nports:\n  - 3000 # first\n  - 3001 # second\n  - 3002\nextra: keep\n"
    );
}

#[test]
fn derive_rewrites_existing_nested_flow_collection_field() {
    let mut doc = YamlDoc::parse("host: localhost\nmatrix: [[0]]\n").expect("valid YAML");
    let mut config = MatrixConfig::from_yaml_doc(&doc).expect("derive reads flow matrix");

    config.matrix = vec![vec![1, 2], vec![3]];
    config
        .apply_to_yaml_doc(&mut doc)
        .expect("derive rewrites flow matrix");

    assert_eq!(doc.to_string(), "host: localhost\nmatrix: [[1, 2], [3]]\n");
    doc.commit_edits().expect("flow matrix commits");
    let read = MatrixConfig::from_yaml_doc(&doc).expect("derive reads updated flow matrix");
    assert_eq!(read, config);
}

#[test]
fn derive_writes_appended_empty_mapping_document() {
    let mut doc = YamlDoc::parse("host: first\nport: 1000\n").expect("valid YAML");

    doc.append_empty_mapping_document()
        .expect("append empty document queues");
    doc.commit_edits().expect("appended document commits");

    let config = Config {
        host: "second".to_owned(),
        port: 2000,
    };
    doc.write_document(1, &config)
        .expect("derive writes appended document");
    doc.commit_edits().expect("written document commits");

    let read: Config = doc
        .read_document(1)
        .expect("derive reads appended document");
    assert_eq!(read, config);
    assert_eq!(
        doc.to_string(),
        "host: first\nport: 1000\n---\n{host: second, port: 2000}\n"
    );
}

#[test]
fn derive_appends_config_document_directly() {
    let mut doc = YamlDoc::parse("host: first\nport: 1000\n").expect("valid YAML");
    let config = Config {
        host: "second".to_owned(),
        port: 2000,
    };

    doc.append_document(&config)
        .expect("derive config document append queues");
    doc.commit_edits().expect("appended config commits");

    let read: Config = doc.read_document(1).expect("derive reads appended config");
    assert_eq!(read, config);
}
