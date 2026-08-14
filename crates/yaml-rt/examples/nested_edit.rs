use yaml_rt::{FromYamlDoc, ToYamlDoc, YamlDoc, YamlError, YamlRt};

#[derive(Debug, PartialEq, Eq, YamlRt)]
struct Server {
    host: String,

    #[yaml(default = 8080)]
    port: u16,
}

#[derive(Debug, PartialEq, Eq, YamlRt)]
struct Config {
    name: String,
    server: Server,
    ports: Vec<u16>,
}

fn main() -> Result<(), YamlError> {
    let input = r#"name: app
server:
  # selected host
  host: "localhost" # inline
  extra: keep
ports:
  - 8080 # public
  - 9090 # admin
"#;
    let mut doc = YamlDoc::parse(input)?;
    let mut config = Config::from_yaml_doc(&doc)?;

    println!("original:\n{}", doc.as_source());
    println!("decoded: {config:#?}");

    "example.com".clone_into(&mut config.server.host);
    config.server.port = 9443;
    config.ports = vec![3000, 3001, 3002];
    config.apply_to_yaml_doc(&mut doc)?;

    println!("edited:\n{doc}");

    Ok(())
}
