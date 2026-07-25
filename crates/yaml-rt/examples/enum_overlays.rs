use yaml_rt::{FromYamlDoc, ToYamlDoc, YamlDoc, YamlError, YamlRoundTrip};

#[derive(Debug, PartialEq, Eq, YamlRoundTrip)]
struct Port(u16);

#[derive(Debug, PartialEq, Eq, YamlRoundTrip)]
#[yaml(rename_all = "lowercase")]
enum Mode {
    Automatic,
    Port(Port),
    Server {
        host: String,
        #[yaml(default = 8080)]
        port: u16,
    },
}

#[derive(Debug, PartialEq, Eq, YamlRoundTrip)]
struct Config {
    mode: Mode,
}

fn main() -> Result<(), YamlError> {
    let input = "\
mode: !server
  host: api # selected endpoint
  port: 8080
  extension: keep
";
    let mut doc = YamlDoc::parse(input)?;
    let mut config = Config::from_yaml_doc(&doc)?;

    if let Mode::Server { host, port } = &mut config.mode {
        *host = "web".to_owned();
        *port = 9090;
    }
    config.apply_to_yaml_doc(&mut doc)?;

    assert_eq!(
        doc.to_string(),
        "\
mode: !server
  host: web # selected endpoint
  port: 9090
  extension: keep
"
    );
    Ok(())
}
