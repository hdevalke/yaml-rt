use serde::{Deserialize, Serialize};
use yaml_rt::{Deserializer, Serializer, from_str, to_string};

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Config {
    host: String,
    port: u16,
}

#[test]
fn serde_feature_reexports_the_adapter_api() {
    let input = "host: localhost\nport: 8080\n";
    let config: Config = from_str(input).expect("deserialize through facade");
    assert_eq!(
        config,
        Config {
            host: "localhost".to_owned(),
            port: 8080,
        }
    );
    assert_eq!(to_string(&config).unwrap(), input);

    let mut output = Vec::new();
    let mut serializer = Serializer::new(&mut output);
    config.serialize(&mut serializer).unwrap();
    assert_eq!(output, input.as_bytes());

    let deserializer = Deserializer::from_str(input);
    assert_eq!(Config::deserialize(deserializer).unwrap(), config);
}
