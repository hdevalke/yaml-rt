use std::fs;
use std::path::Path;

use yaml_rt_schema::{Schema, Value};

#[test]
fn draft2020_12_official_suite() {
    let root = std::env::var("JSON_SCHEMA_TEST_SUITE_DIR").unwrap_or_else(|_| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../third_party/json-schema-test-suite"
        )
        .to_owned()
    });
    let root = Path::new(&root).join("tests/draft2020-12");
    let mut failures = Vec::new();
    let mut cases = 0;
    for entry in fs::read_dir(&root).unwrap() {
        let entry = entry.unwrap();
        if entry.path().extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let file = entry.file_name().to_string_lossy().into_owned();
        if matches!(file.as_str(), "refRemote.json" | "vocabulary.json") {
            continue;
        }
        let groups: Value = Value::parse(&fs::read_to_string(entry.path()).unwrap()).unwrap();
        for group in groups.as_array().unwrap() {
            let schema = group["schema"].to_string();
            let Ok(mut schema) = Schema::parse(&schema, None) else {
                failures.push(format!("{file}: {}: schema rejected", group["description"]));
                continue;
            };
            for resource in ["tree", "extendible-dynamic-ref", "detached-dynamicref"] {
                let path =
                    Path::new(&root).join(format!("../../remotes/draft2020-12/{resource}.json"));
                let source = fs::read_to_string(path).unwrap();
                schema
                    .register_resource(
                        &format!("http://localhost:1234/draft2020-12/{resource}.json"),
                        &source,
                    )
                    .unwrap();
            }
            for case in group["tests"].as_array().unwrap() {
                cases += 1;
                let result = schema.validate_json(&case["data"]);
                let actual = result.is_ok();
                if actual != case["valid"].as_bool().unwrap() {
                    failures.push(format!(
                        "{file}: {}: {}: {:?}",
                        group["description"],
                        case["description"],
                        result.err().map(|e| e.to_string())
                    ));
                }
            }
        }
    }
    eprintln!("{cases} official cases, {} failures", failures.len());
    if !failures.is_empty() {
        panic!("{}", failures.join("\n"));
    }
}

#[test]
fn draft2020_12_format_assertions() {
    let root = std::env::var("JSON_SCHEMA_TEST_SUITE_DIR").unwrap_or_else(|_| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../third_party/json-schema-test-suite"
        )
        .to_owned()
    });
    let root = Path::new(&root).join("tests/draft2020-12/optional/format");
    let mut failures = Vec::new();
    let mut cases = 0;
    for entry in fs::read_dir(&root).unwrap() {
        let entry = entry.unwrap();
        let file = entry.file_name().to_string_lossy().into_owned();
        if file == "unknown.json" || file == "ecmascript-regex.json" {
            continue;
        }
        let groups: Value = Value::parse(&fs::read_to_string(entry.path()).unwrap()).unwrap();
        for group in groups.as_array().unwrap() {
            let schema = group["schema"].to_string();
            let schema = Schema::parse(&schema, None)
                .unwrap()
                .with_format_assertions(true);
            for case in group["tests"].as_array().unwrap() {
                cases += 1;
                let actual = schema.validate_json(&case["data"]).is_ok();
                if actual != case["valid"].as_bool().unwrap() {
                    failures.push(format!(
                        "{file}: {}: {}",
                        group["description"], case["description"]
                    ));
                }
            }
        }
    }
    eprintln!("{cases} format cases, {} failures", failures.len());
    if !failures.is_empty() {
        panic!("{}", failures.join("\n"));
    }
}

#[test]
fn draft2020_12_big_numbers() {
    let root = std::env::var("JSON_SCHEMA_TEST_SUITE_DIR").unwrap_or_else(|_| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../third_party/json-schema-test-suite"
        )
        .to_owned()
    });
    let path = Path::new(&root).join("tests/draft2020-12/optional/bignum.json");
    let groups: Value = Value::parse(&fs::read_to_string(path).unwrap()).unwrap();
    let mut failures = Vec::new();
    for group in groups.as_array().unwrap() {
        let schema = Schema::parse(&group["schema"].to_string(), None).unwrap();
        for case in group["tests"].as_array().unwrap() {
            let actual = schema.validate_json(&case["data"]).is_ok();
            if actual != case["valid"].as_bool().unwrap() {
                failures.push(format!("{}: {}", group["description"], case["description"]));
            }
        }
    }
    if !failures.is_empty() {
        panic!("{}", failures.join("\n"));
    }
}

#[test]
fn draft2020_12_optional_local_cases() {
    let root = std::env::var("JSON_SCHEMA_TEST_SUITE_DIR").unwrap_or_else(|_| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../third_party/json-schema-test-suite"
        )
        .to_owned()
    });
    let root = Path::new(&root).join("tests/draft2020-12/optional");
    let files = [
        "anchor.json",
        "dynamicRef.json",
        "id.json",
        "no-schema.json",
        "unknownKeyword.json",
        "refOfUnknownKeyword.json",
        "float-overflow.json",
        "non-bmp-regex.json",
    ];
    let mut failures = Vec::new();
    for file in files {
        let groups: Value = Value::parse(&fs::read_to_string(root.join(file)).unwrap()).unwrap();
        for group in groups.as_array().unwrap() {
            let source = group["schema"].to_string();
            let Ok(schema) = Schema::parse(&source, None) else {
                failures.push(format!("{file}: {}: schema rejected", group["description"]));
                continue;
            };
            for case in group["tests"].as_array().unwrap() {
                let actual = schema.validate_json(&case["data"]).is_ok();
                if actual != case["valid"].as_bool().unwrap() {
                    failures.push(format!(
                        "{file}: {}: {}",
                        group["description"], case["description"]
                    ));
                }
            }
        }
    }
    if !failures.is_empty() {
        panic!("{}", failures.join("\n"));
    }
}
