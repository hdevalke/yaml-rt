use yaml_rt::{FromYamlDoc, ToYamlDoc, YamlDoc, YamlError, YamlRt};

#[derive(Debug, PartialEq, Eq, YamlRt)]
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

fn main() -> Result<(), YamlError> {
    let input = r#"# main server
host: "localhost"

# chosen port
port: 3000

extra: keep-me
"#;
    let mut doc = YamlDoc::parse(input)?;
    let mut config = Config::from_yaml_doc(&doc)?;

    println!("original:\n{}", doc.as_source());
    println!("decoded: {config:#?}");

    config.port = 9090;
    config.debug = true;
    config.apply_to_yaml_doc(&mut doc)?;

    println!("edited:\n{doc}");

    Ok(())
}
