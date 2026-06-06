use yaml_rt::{FromYamlDoc, ToYamlDoc, YamlDoc, YamlError, YamlRoundTrip};

#[derive(Debug, PartialEq, Eq, YamlRoundTrip)]
struct Config {
    /// Server hostname.
    host: String,

    /// Server port.
    #[yaml(default = 8080)]
    port: u16,

    /// Enable debug logging.
    #[yaml(default = false)]
    debug: bool,
}

#[test]
fn typed_overlay_usefulness_target_preserves_comments_unknown_fields_and_style()
-> Result<(), YamlError> {
    let input =
        "# main server\nhost: \"localhost\"\n\n# chosen port\nport: 3000\n\nextra: keep-me\n";
    let mut doc = YamlDoc::parse(input)?;
    let mut cfg = Config::from_yaml_doc(&doc)?;

    assert_eq!(
        cfg,
        Config {
            host: "localhost".to_owned(),
            port: 3000,
            debug: false,
        }
    );

    cfg.port = 9090;
    cfg.debug = true;
    cfg.apply_to_yaml_doc(&mut doc)?;

    assert_eq!(
        doc.to_string(),
        "# main server\nhost: \"localhost\"\n\n# chosen port\nport: 9090\n\nextra: keep-me\n\n# Enable debug logging.\ndebug: true\n"
    );

    Ok(())
}
