use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use yaml_rt_schema::{Schema, Value};

const CORE_EXCLUSIONS: &[(&str, &str)] = &[
    (
        "refRemote.json",
        "remote fetching is outside the release scope",
    ),
    ("vocabulary.json", "custom meta-schemas are unsupported"),
];
const FORMAT_EXCLUSIONS: &[(&str, &str)] = &[
    (
        "unknown.json",
        "tests annotation behavior rather than assertions",
    ),
    (
        "ecmascript-regex.json",
        "Rust regex grammar differs from ECMAScript",
    ),
];
const REMOTE_FIXTURES: &[&str] = &["tree", "extendible-dynamic-ref", "detached-dynamicref"];

fn suite_root() -> PathBuf {
    std::env::var_os("JSON_SCHEMA_TEST_SUITE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/json-schema-test-suite")
        })
}

fn json_files(directory: &Path, exclusions: &[(&str, &str)]) -> Vec<PathBuf> {
    let mut paths: Vec<_> = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("{}: {error}", directory.display()))
        .map(|entry| entry.expect("read suite directory entry").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .filter(|path| {
            !exclusions
                .iter()
                .any(|(file, _)| path.file_name().is_some_and(|name| name == *file))
        })
        .collect();
    paths.sort();
    for (file, reason) in exclusions {
        eprintln!("excluded {file}: {reason}");
    }
    assert!(!paths.is_empty(), "{}: no suite files", directory.display());
    paths
}

// Inspect reference keywords, including those in nested schemas. Relative
// references use the fixture filename under the official localhost base URI.
fn references_fixture(value: &Value, filename: &str) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(keyword, value)| {
            (matches!(keyword.as_str(), "$ref" | "$dynamicRef")
                && value.as_str().is_some_and(|reference| {
                    let resource = reference.split('#').next().unwrap();
                    resource == filename
                        || resource
                            .strip_suffix(filename)
                            .is_some_and(|base| base == "http://localhost:1234/draft2020-12/")
                }))
                || references_fixture(value, filename)
        }),
        Value::Array(values) => values
            .iter()
            .any(|value| references_fixture(value, filename)),
        _ => false,
    }
}

#[test]
fn fixture_selection_handles_nested_references_and_fragments() {
    for source in [
        r##"{"$ref":"tree.json#node"}"##,
        r##"{"allOf":[{"$defs":{"node":{"$dynamicRef":"http://localhost:1234/draft2020-12/tree.json#node"}}}]}"##,
    ] {
        assert!(references_fixture(
            &Value::parse(source).unwrap(),
            "tree.json"
        ));
    }
    for source in [
        r#"{"$id":"http://localhost:1234/draft2020-12/tree.json"}"#,
        r#"{"$ref":"http://example.com/tree.json"}"#,
        r#"{"$ref":"other-tree.json"}"#,
        r##"{"$dynamicRef":"#node"}"##,
    ] {
        assert!(!references_fixture(
            &Value::parse(source).unwrap(),
            "tree.json"
        ));
    }
}

fn run_suite(label: &str, files: Vec<PathBuf>, formats: bool, remotes: bool) {
    assert!(!files.is_empty(), "{label}: no suite files");
    let started = Instant::now();
    let root = suite_root();
    let resources: Vec<_> = if remotes {
        REMOTE_FIXTURES
            .iter()
            .map(|name| {
                let filename = format!("{name}.json");
                let path = root.join("remotes/draft2020-12").join(&filename);
                let source = fs::read_to_string(&path)
                    .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
                (filename, source)
            })
            .collect()
    } else {
        Vec::new()
    };
    let mut failures = Vec::new();
    let mut cases = 0;
    let mut groups_run = 0;
    let file_count = files.len();
    for path in files {
        let file = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .display()
            .to_string();
        let source = fs::read_to_string(&path).unwrap_or_else(|error| panic!("{file}: {error}"));
        let groups = Value::parse(&source)
            .unwrap_or_else(|error| panic!("{file}: invalid fixture: {error}"));
        let groups = groups
            .as_array()
            .unwrap_or_else(|| panic!("{file}: expected groups array"));
        assert!(!groups.is_empty(), "{file}: no groups");
        for (group_index, group) in groups.iter().enumerate() {
            groups_run += 1;
            let context = format!("{file}: group {group_index} ({})", group["description"]);
            let tests = group["tests"]
                .as_array()
                .unwrap_or_else(|| panic!("{context}: expected tests array"));
            assert!(!tests.is_empty(), "{context}: no cases");
            cases += tests.len();
            let mut schema = match Schema::parse(&group["schema"].to_string(), None) {
                Ok(schema) => schema.with_format_assertions(formats),
                Err(error) => {
                    failures.push(format!("{context}: schema rejected: {error}"));
                    continue;
                }
            };
            for (filename, source) in &resources {
                if references_fixture(&group["schema"], filename) {
                    schema
                        .register_resource(
                            &format!("http://localhost:1234/draft2020-12/{filename}"),
                            source,
                        )
                        .unwrap_or_else(|error| panic!("{context}: resource {filename}: {error}"));
                }
            }
            for (case_index, case) in tests.iter().enumerate() {
                let expected = case["valid"].as_bool().unwrap_or_else(|| {
                    panic!("{context}: case {case_index}: expected valid boolean")
                });
                let result = schema.validate_json(&case["data"]);
                if result.is_ok() != expected {
                    let error = result.err().map_or_else(
                        || "accepted invalid instance".to_owned(),
                        |error| error.to_string(),
                    );
                    failures.push(format!(
                        "{context}: case {case_index} ({}): expected valid={expected}: {error}",
                        case["description"],
                    ));
                }
            }
        }
    }
    eprintln!(
        "{label}: {file_count} files, {groups_run} groups, {cases} cases, {} failures in {:.2?}",
        failures.len(),
        started.elapsed()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn draft2020_12_official_suite() {
    run_suite(
        "core",
        json_files(&suite_root().join("tests/draft2020-12"), CORE_EXCLUSIONS),
        false,
        true,
    );
}

#[test]
fn draft2020_12_format_assertions() {
    run_suite(
        "formats",
        json_files(
            &suite_root().join("tests/draft2020-12/optional/format"),
            FORMAT_EXCLUSIONS,
        ),
        true,
        false,
    );
}

#[test]
fn draft2020_12_big_numbers() {
    run_suite(
        "big numbers",
        vec![suite_root().join("tests/draft2020-12/optional/bignum.json")],
        false,
        false,
    );
}

#[test]
fn draft2020_12_optional_local_cases() {
    let root = suite_root().join("tests/draft2020-12/optional");
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
    run_suite(
        "optional local",
        files.iter().map(|file| root.join(file)).collect(),
        false,
        false,
    );
}
