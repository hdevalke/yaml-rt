//! Versioned, browser-local playground scenarios.

use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    version: u32,
    source: String,
    input_schema: String,
    output_schema: String,
    command: String,
    document_index: usize,
    selector_kind: String,
    selector: String,
    from: String,
    destination: String,
    value: String,
    new_key: String,
    patch: String,
}

impl Snapshot {
    fn validate(&self) -> Result<(), String> {
        if self.version != VERSION {
            return Err(format!(
                "Unsupported playground snapshot version: {}",
                self.version
            ));
        }
        if ![
            "validate",
            "schema",
            "query",
            "get",
            "add",
            "remove",
            "replace",
            "rename-key",
            "move",
            "copy",
            "test",
            "patch",
        ]
        .contains(&self.command.as_str())
        {
            return Err(format!("Unknown operation: {}", self.command));
        }
        if !["pointer", "jsonpath"].contains(&self.selector_kind.as_str()) {
            return Err(format!("Unknown selector kind: {}", self.selector_kind));
        }
        if self.command == "query" && self.selector_kind != "jsonpath" {
            return Err("The query operation requires a JSONPath selector".to_owned());
        }
        Ok(())
    }
}

pub fn encode(snapshot: &Snapshot) -> Result<String, String> {
    snapshot.validate()?;
    yaml_rt_serde::to_string(snapshot).map_err(|error| error.to_string())
}

pub fn decode(source: &str) -> Result<Snapshot, String> {
    let snapshot: Snapshot = yaml_rt_serde::from_str(source).map_err(|error| error.to_string())?;
    snapshot.validate()?;
    Ok(snapshot)
}

/// A validated scenario whose fields are readable from JavaScript.
#[wasm_bindgen]
pub struct WasmSnapshot(Snapshot);

#[wasm_bindgen]
impl WasmSnapshot {
    #[wasm_bindgen(getter)]
    pub fn source(&self) -> String {
        self.0.source.clone()
    }
    #[wasm_bindgen(getter)]
    pub fn input_schema(&self) -> String {
        self.0.input_schema.clone()
    }
    #[wasm_bindgen(getter)]
    pub fn output_schema(&self) -> String {
        self.0.output_schema.clone()
    }
    #[wasm_bindgen(getter)]
    pub fn command(&self) -> String {
        self.0.command.clone()
    }
    #[wasm_bindgen(getter)]
    pub fn document_index(&self) -> usize {
        self.0.document_index
    }
    #[wasm_bindgen(getter)]
    pub fn selector_kind(&self) -> String {
        self.0.selector_kind.clone()
    }
    #[wasm_bindgen(getter)]
    pub fn selector(&self) -> String {
        self.0.selector.clone()
    }
    #[wasm_bindgen(getter)]
    pub fn from(&self) -> String {
        self.0.from.clone()
    }
    #[wasm_bindgen(getter)]
    pub fn destination(&self) -> String {
        self.0.destination.clone()
    }
    #[wasm_bindgen(getter)]
    pub fn value(&self) -> String {
        self.0.value.clone()
    }
    #[wasm_bindgen(getter)]
    pub fn new_key(&self) -> String {
        self.0.new_key.clone()
    }
    #[wasm_bindgen(getter)]
    pub fn patch(&self) -> String {
        self.0.patch.clone()
    }
}

#[wasm_bindgen]
#[allow(clippy::too_many_arguments)]
pub fn export_snapshot(
    source: String,
    input_schema: String,
    output_schema: String,
    command: String,
    document_index: usize,
    selector_kind: String,
    selector: String,
    from: String,
    destination: String,
    value: String,
    new_key: String,
    patch: String,
) -> Result<String, JsValue> {
    encode(&Snapshot {
        version: VERSION,
        source,
        input_schema,
        output_schema,
        command,
        document_index,
        selector_kind,
        selector,
        from,
        destination,
        value,
        new_key,
        patch,
    })
    .map_err(|error| JsValue::from_str(&error))
}

#[wasm_bindgen]
pub fn import_snapshot(source: &str) -> Result<WasmSnapshot, JsValue> {
    decode(source)
        .map(WasmSnapshot)
        .map_err(|error| JsValue::from_str(&error))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Snapshot {
        Snapshot {
            version: VERSION,
            source: "# broken YAML\r\na: [\r\n☃\n".to_owned(),
            input_schema: "type: [\r\n".to_owned(),
            output_schema: "\n  unusual: 'value'\n".to_owned(),
            command: "replace".to_owned(),
            document_index: 2,
            selector_kind: "jsonpath".to_owned(),
            selector: "$.a[?@.name == 'x']".to_owned(),
            from: "/hidden/source".to_owned(),
            destination: "/hidden/destination".to_owned(),
            value: "# value\n- a: b\n".to_owned(),
            new_key: "old: key".to_owned(),
            patch: "- op: test\n  value: null\n".to_owned(),
        }
    }

    #[test]
    fn round_trips_every_field_as_exact_text() {
        let snapshot = sample();
        assert_eq!(decode(&encode(&snapshot).unwrap()).unwrap(), snapshot);
    }

    #[test]
    fn round_trips_default_playground_source() {
        let mut snapshot = sample();
        snapshot.source = "# Production services — comments and style stay put\nservices:\n  - name: api\n    port: 8080 # public endpoint\n    enabled: TRUE\n".to_owned();
        assert_eq!(decode(&encode(&snapshot).unwrap()).unwrap(), snapshot);
    }

    #[test]
    fn round_trips_multiple_hashes_and_escaped_quotes() {
        let mut snapshot = sample();
        snapshot.source = "# Production ser # vices — comments and style stay put\nsn  - name # api\n    port: \"8080 # public endpoint\"\n    enabled: TRUE\n".to_owned();
        assert_eq!(decode(&encode(&snapshot).unwrap()).unwrap(), snapshot);
    }

    #[test]
    fn round_trips_empty_fields() {
        let mut snapshot = sample();
        snapshot.source.clear();
        snapshot.input_schema.clear();
        snapshot.output_schema.clear();
        snapshot.selector.clear();
        snapshot.from.clear();
        snapshot.destination.clear();
        snapshot.value.clear();
        snapshot.new_key.clear();
        snapshot.patch.clear();
        assert_eq!(decode(&encode(&snapshot).unwrap()).unwrap(), snapshot);
    }

    #[test]
    fn rejects_invalid_or_incomplete_snapshots() {
        let yaml = encode(&sample()).unwrap();
        assert!(
            decode(&yaml.replace("version: 1", "version: 2"))
                .unwrap_err()
                .contains("Unsupported")
        );
        assert!(
            decode(&yaml.replace("command: replace", "command: unknown"))
                .unwrap_err()
                .contains("Unknown operation")
        );
        assert!(
            decode(&yaml.replace("selector_kind: jsonpath", "selector_kind: unknown"))
                .unwrap_err()
                .contains("Unknown selector kind")
        );
        assert!(decode("version: 1\nsource: only\n").is_err());
        assert!(decode("version: [\n").is_err());
    }
}
