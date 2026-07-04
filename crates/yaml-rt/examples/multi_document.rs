use yaml_rt::{YamlDoc, YamlError, YamlRoundTrip};

#[derive(Debug, PartialEq, Eq, YamlRoundTrip)]
struct Config {
    host: String,
    port: u16,
}

fn main() -> Result<(), YamlError> {
    let input = r#"---
host: first
port: 1000
---
# selected
host: "second"
port: 2000
extra: keep
"#;
    let mut doc = YamlDoc::parse(input)?;
    let mut selected: Config = doc.read_document(1)?;

    println!("original stream:\n{}", doc.as_source());
    println!("selected document: {selected:#?}");

    selected.port = 9090;
    doc.write_document(1, &selected)?;

    println!("edited stream:\n{}", doc);

    Ok(())
}
