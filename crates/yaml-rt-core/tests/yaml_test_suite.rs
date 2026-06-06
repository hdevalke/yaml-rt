use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use yaml_rt_core::YamlDoc;

/// Environment variable overriding the in-repo YAML Test Suite data checkout.
const SUITE_DIR_ENV: &str = "YAML_TEST_SUITE_DIR";
/// Optional comma-separated list of case ids to run, such as `MJS9` or `VJP3:00`.
const CASES_ENV: &str = "YAML_TEST_SUITE_CASES";
/// Set to `1` to run every discovered case. This is intentionally opt-in while
/// the parser is still an MVP subset.
const RUN_ALL_ENV: &str = "YAML_TEST_SUITE_RUN_ALL";

#[derive(Debug, Clone, PartialEq, Eq)]
struct SuiteCase {
    id: String,
    dir: PathBuf,
    input: PathBuf,
    is_error: bool,
}

#[test]
fn yaml_test_suite_data_harness() {
    let root = suite_root();

    let selected = selected_cases();
    let run_all = env::var_os(RUN_ALL_ENV).is_some_and(|value| value == "1");
    if selected.is_empty() && !run_all {
        eprintln!(
            "set {CASES_ENV}=CASE[,CASE...] for a focused run, or {RUN_ALL_ENV}=1 to run every discovered YAML Test Suite case"
        );
        return;
    }

    let cases = discover_cases(&root).unwrap_or_else(|error| {
        panic!(
            "failed to discover YAML Test Suite cases below {}: {error}",
            root.display()
        )
    });
    assert!(
        !cases.is_empty(),
        "no YAML Test Suite cases with in.yaml found below {}",
        root.display()
    );

    let mut failures = Vec::new();
    let mut ran = 0usize;
    for case in cases {
        if !run_all && !selected.iter().any(|selected| selected == &case.id) {
            continue;
        }
        ran += 1;
        if let Err(error) = run_case(&case) {
            failures.push(format!("{} ({}): {error}", case.id, case.dir.display()));
        }
    }

    assert!(
        ran > 0,
        "no YAML Test Suite cases matched {CASES_ENV}={}",
        selected.join(",")
    );

    if !failures.is_empty() {
        panic!(
            "{} YAML Test Suite case(s) failed:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}

fn suite_root() -> PathBuf {
    let root = env::var_os(SUITE_DIR_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join("third_party")
                .join("yaml-test-suite")
        });

    if !root.is_dir() {
        panic!(
            "YAML Test Suite data directory {} does not exist; initialize the submodule with `git submodule update --init --recursive` or set {SUITE_DIR_ENV}",
            root.display()
        );
    }

    let data = root.join("data");
    if data.is_dir() { data } else { root }
}

fn selected_cases() -> Vec<String> {
    env::var(CASES_ENV)
        .ok()
        .into_iter()
        .flat_map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|case| !case.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .collect()
}

fn discover_cases(root: &Path) -> std::io::Result<Vec<SuiteCase>> {
    let mut cases = Vec::new();
    discover_cases_inner(root, &mut cases)?;
    cases.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(cases)
}

fn discover_cases_inner(dir: &Path, cases: &mut Vec<SuiteCase>) -> std::io::Result<()> {
    let input = dir.join("in.yaml");
    if input.is_file() {
        cases.push(SuiteCase {
            id: case_id(dir),
            is_error: dir.join("error").is_file(),
            input,
            dir: dir.to_owned(),
        });
        return Ok(());
    }

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            discover_cases_inner(&entry.path(), cases)?;
        }
    }

    Ok(())
}

fn case_id(dir: &Path) -> String {
    let name = dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let parent = dir
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or_default();

    if name.len() == 2 && name.bytes().all(|byte| byte.is_ascii_digit()) {
        format!("{parent}:{name}")
    } else {
        name.to_owned()
    }
}

fn run_case(case: &SuiteCase) -> Result<(), String> {
    let input = fs::read_to_string(&case.input)
        .map_err(|error| format!("failed to read {}: {error}", case.input.display()))?;
    let parsed = YamlDoc::parse(&input);

    if case.is_error {
        if parsed.is_ok() {
            return Err("expected parse error, but parser accepted the case".to_owned());
        }
        return Ok(());
    }

    let doc = parsed.map_err(|error| format!("expected valid parse: {error}"))?;
    let output = doc.to_string();
    if output != input {
        return Err("valid case did not round-trip byte-identically".to_owned());
    }

    Ok(())
}
