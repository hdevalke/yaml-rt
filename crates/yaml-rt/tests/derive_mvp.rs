use yaml_rt::{FromYamlDoc, ToYamlDoc, YamlDoc, YamlRoundTrip};

#[derive(Debug, PartialEq, Eq, YamlRoundTrip)]
struct Config {
    host: String,
    port: u16,
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
