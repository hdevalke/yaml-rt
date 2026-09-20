use std::collections::BTreeMap;

use crate::value::{Map, Value};
use yaml_rt_core::YamlDoc;

use crate::{DIALECT, Error, model};

/// Infers a permissive JSON Schema from one YAML document.
pub fn generate_schema(doc: &YamlDoc, document: usize) -> Result<Value, Error> {
    let instance = model::from_document(doc, document)?;
    let mut schema = infer(&instance.value);
    if let Value::Object(object) = &mut schema {
        object.insert("$schema".into(), Value::String(DIALECT.into()));
    }
    Ok(schema)
}

fn infer(value: &Value) -> Value {
    match value {
        Value::Null => typed("null"),
        Value::Bool(_) => typed("boolean"),
        Value::Number(_) => typed(if crate::numeric::is_integer(value) {
            "integer"
        } else {
            "number"
        }),
        Value::String(_) => typed("string"),
        Value::Array(items) => {
            let mut object = Map::new();
            object.insert("type".into(), Value::String("array".into()));
            if !items.is_empty() {
                object.insert("items".into(), combine(items.iter().map(infer).collect()));
            }
            Value::Object(object)
        }
        Value::Object(properties) => {
            let mut names = BTreeMap::new();
            for (name, value) in properties {
                names.insert(name.clone(), infer(value));
            }
            {
                let mut schema = typed("object");
                if let Value::Object(object) = &mut schema {
                    object.insert("properties".into(), Value::Object(names));
                }
                schema
            }
        }
    }
}

fn typed(kind: &str) -> Value {
    let mut object = Map::new();
    object.insert("type".into(), Value::String(kind.into()));
    Value::Object(object)
}

fn combine(mut schemas: Vec<Value>) -> Value {
    schemas.sort_by_key(Value::to_string);
    schemas.dedup();
    if schemas.len() == 1 {
        schemas.remove(0)
    } else {
        {
            let mut object = Map::new();
            object.insert("anyOf".into(), Value::Array(schemas));
            Value::Object(object)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inferred_objects_are_open_and_optional() {
        let doc = YamlDoc::parse("name: Ada\nitems: [1, null]\n").unwrap();
        let schema = generate_schema(&doc, 0).unwrap();
        assert!(schema.get("required").is_none());
        assert!(schema.get("additionalProperties").is_none());
        assert_eq!(
            schema["properties"]["items"]["items"]["anyOf"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }
}
