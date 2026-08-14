use std::time::Duration;

use yaml_rt::{FromYamlDoc, ToYamlDoc, YamlDoc, YamlError, YamlRt};

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
struct Server {
    host: String,
    port: u16,
}

#[derive(Debug, PartialEq, Eq, YamlRt)]
struct Group<T, const N: usize> {
    name: String,
    values: [T; N],
}

#[derive(Debug, PartialEq, Eq, YamlRt)]
struct Config {
    #[yaml(with = "duration_seconds", rename = "timeout-seconds")]
    timeout: Duration,
    servers: Vec<Server>,
    fallback: Option<Server>,
    groups: Vec<Group<u16, 2>>,
}

fn main() -> Result<(), YamlError> {
    let input = "\
{timeout-seconds: 30, servers: [{host: api, port: 80, note: keep}], fallback: null, groups: [{name: blue, values: [1, 2]}], extra: keep}
";
    let mut doc = YamlDoc::parse(input)?;
    let mut config = Config::from_yaml_doc(&doc)?;

    config.servers[0].port = 8080;
    config.servers.push(Server {
        host: "worker".to_owned(),
        port: 9000,
    });
    config.apply_to_yaml_doc(&mut doc)?;

    assert_eq!(
        doc.to_string(),
        "\
{timeout-seconds: 30, servers: [{host: api, port: 8080, note: keep}, {host: worker, port: 9000}], fallback: null, groups: [{name: blue, values: [1, 2]}], extra: keep}
"
    );
    Ok(())
}
