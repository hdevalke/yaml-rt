use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_dir() -> PathBuf {
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path =
        std::env::temp_dir().join(format!("yaml-rt-cli-test-{}-{counter}", std::process::id()));
    fs::create_dir(&path).unwrap();
    path
}

#[test]
fn help_and_version_report_distribution_metadata() {
    let version = Command::new(env!("CARGO_BIN_EXE_yaml-rt"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(version.status.success(), "{:?}", version.stderr);
    assert_eq!(
        version.stdout,
        format!("yaml-rt {}\n", env!("CARGO_PKG_VERSION")).as_bytes()
    );
    assert!(version.stderr.is_empty());

    let help = Command::new(env!("CARGO_BIN_EXE_yaml-rt"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(help.status.success(), "{:?}", help.stderr);
    assert!(String::from_utf8_lossy(&help.stdout).contains("Usage: yaml-rt"));
    assert!(help.stderr.is_empty());

    for operation in ["get", "add", "remove", "replace", "rename-key", "test"] {
        let help = Command::new(env!("CARGO_BIN_EXE_yaml-rt"))
            .args([operation, "--help"])
            .output()
            .unwrap();
        assert!(help.status.success(), "{:?}", help.stderr);
        assert!(String::from_utf8_lossy(&help.stdout).contains("--query <QUERY>"));
    }

    for operation in ["move", "copy", "patch"] {
        let help = Command::new(env!("CARGO_BIN_EXE_yaml-rt"))
            .args([operation, "--help"])
            .output()
            .unwrap();
        assert!(help.status.success(), "{:?}", help.stderr);
        assert!(!String::from_utf8_lossy(&help.stdout).contains("--query"));
    }
}

#[test]
fn directory_targets_recurse_with_sorted_path_headers() {
    let directory = temp_dir();
    fs::create_dir_all(directory.join(".hidden")).unwrap();
    fs::create_dir_all(directory.join("nested")).unwrap();
    fs::write(directory.join("a.yaml"), "name: a\n").unwrap();
    fs::write(directory.join(".hidden/c.yml"), "name: c\n").unwrap();
    fs::write(directory.join("nested/b.YAML"), "name: b\n").unwrap();
    fs::write(
        directory.join("nested/ignored.json"),
        "{\"name\":\"ignored\"}\n",
    )
    .unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(directory.join("a.yaml"), directory.join("linked.yml")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_yaml-rt"))
        .args(["get", "/name", directory.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    assert_eq!(
        output.stdout,
        b"==> .hidden/c.yml <==\nc\n\n==> a.yaml <==\na\n\n==> nested/b.YAML <==\nb\n"
    );
    assert!(output.stderr.is_empty());

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn omitted_target_scans_current_directory_and_empty_batches_succeed() {
    let directory = temp_dir();
    fs::create_dir(directory.join("nested")).unwrap();
    fs::write(directory.join("one.yaml"), "value: 1\n").unwrap();
    fs::write(directory.join("nested/two.yml"), "other: 2\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_yaml-rt"))
        .current_dir(&directory)
        .args(["query", "$.value"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    assert_eq!(output.stdout, b"==> one.yaml <==\n\"/value\": 1\n");

    let output = Command::new(env!("CARGO_BIN_EXE_yaml-rt"))
        .current_dir(&directory)
        .args(["query", "$.missing"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    assert!(output.stdout.is_empty());

    let empty = directory.join("empty");
    fs::create_dir(&empty).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_yaml-rt"))
        .args(["query", "$", empty.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn batch_read_output_is_combined_and_cannot_overwrite_an_input() {
    let directory = temp_dir();
    let first = directory.join("first.yaml");
    let second = directory.join("second.yml");
    let result = directory.join("result.txt");
    fs::write(&first, "value: first\n").unwrap();
    fs::write(&second, "value: second\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_yaml-rt"))
        .args([
            "get",
            "/value",
            "--output",
            result.to_str().unwrap(),
            directory.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    assert!(output.stdout.is_empty());
    assert_eq!(
        fs::read(&result).unwrap(),
        b"==> first.yaml <==\nfirst\n\n==> second.yml <==\nsecond\n"
    );

    let original = fs::read(&first).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_yaml-rt"))
        .args([
            "get",
            "/value",
            "--output",
            first.to_str().unwrap(),
            directory.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("must not name input file"));
    assert_eq!(fs::read(&first).unwrap(), original);

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn batch_mutations_require_in_place_and_continue_after_failures() {
    let directory = temp_dir();
    let invalid = directory.join("bad.yaml");
    let first = directory.join("first.yaml");
    let second = directory.join("second.yml");
    fs::write(&invalid, "[\n").unwrap();
    fs::write(&first, "value: old\n").unwrap();
    fs::write(&second, "value: old\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_yaml-rt"))
        .args([
            "replace",
            "/value",
            "--value",
            "new",
            directory.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("require --in-place"));
    assert_eq!(fs::read(&first).unwrap(), b"value: old\n");

    let output = Command::new(env!("CARGO_BIN_EXE_yaml-rt"))
        .args([
            "replace",
            "/value",
            "--value",
            "new",
            "--in-place",
            directory.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("bad.yaml:"), "{stderr}");
    assert!(
        stderr.contains("processed 3 YAML files: 2 succeeded, 1 failed; 0 traversal errors"),
        "{stderr}"
    );
    assert_eq!(fs::read(&invalid).unwrap(), b"[\n");
    assert_eq!(fs::read(&first).unwrap(), b"value: new\n");
    assert_eq!(fs::read(&second).unwrap(), b"value: new\n");

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn batch_value_and_patch_sources_read_stdin_once() {
    let directory = temp_dir();
    let first = directory.join("first.yaml");
    let second = directory.join("second.yml");
    fs::write(&first, "value: old\n").unwrap();
    fs::write(&second, "value: old\n").unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_yaml-rt"))
        .args([
            "replace",
            "/value",
            "--value-file",
            "-",
            "--in-place",
            directory.to_str().unwrap(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"from-value-file\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    assert_eq!(fs::read(&first).unwrap(), b"value: from-value-file\n");
    assert_eq!(fs::read(&second).unwrap(), b"value: from-value-file\n");

    let mut child = Command::new(env!("CARGO_BIN_EXE_yaml-rt"))
        .args([
            "patch",
            "--patch-file",
            "-",
            "--in-place",
            directory.to_str().unwrap(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"- {op: replace, path: /value, value: from-patch}\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    assert_eq!(fs::read(&first).unwrap(), b"value: from-patch\n");
    assert_eq!(fs::read(&second).unwrap(), b"value: from-patch\n");

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn value_is_never_autodetected_as_a_filename() {
    let directory = temp_dir();
    let input = directory.join("document.yaml");
    let coincidental = directory.join("config.yaml");
    fs::write(&input, "filename: old\n").unwrap();
    fs::write(&coincidental, "from-file\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_yaml-rt"))
        .current_dir(&directory)
        .args([
            "replace",
            "/filename",
            "--value",
            "config.yaml",
            "document.yaml",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    assert_eq!(output.stdout, b"filename: config.yaml\n");

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn value_file_and_document_selection_work() {
    let directory = temp_dir();
    let input = directory.join("document.yaml");
    let value = directory.join("value.yaml");
    fs::write(&input, "---\nname: first\n---\nname: second\n").unwrap();
    fs::write(&value, "{nested: true}\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_yaml-rt"))
        .args([
            "replace",
            "/name",
            "--doc",
            "1",
            "--value-file",
            value.to_str().unwrap(),
            input.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    assert_eq!(
        output.stdout,
        b"---\nname: first\n---\nname: {nested: true}\n"
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn query_supports_document_selection_and_output_files() {
    let directory = temp_dir();
    let input = directory.join("document.yaml");
    let result = directory.join("result.txt");
    fs::write(
        &input,
        "---\nusers: [{name: first}]\n---\nusers: [{name: second}]\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_yaml-rt"))
        .args([
            "query",
            "$.users[*].name",
            "--doc",
            "1",
            "--output",
            result.to_str().unwrap(),
            input.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    assert!(output.stdout.is_empty());
    assert_eq!(
        fs::read(&result).unwrap(),
        b"\"/users/0/name\": \"second\"\n"
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn query_targeted_commands_support_positional_files_and_outputs() {
    let directory = temp_dir();
    let input = directory.join("document.yaml");
    let result = directory.join("result.yaml");
    fs::write(
        &input,
        "---\nusers: [{name: first, active: false}]\n---\nusers: [{name: second, active: false}]\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_yaml-rt"))
        .args([
            "get",
            "--query",
            "$.users[*].name",
            "--doc",
            "1",
            "--output",
            result.to_str().unwrap(),
            input.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    assert!(output.stdout.is_empty());
    assert_eq!(fs::read(&result).unwrap(), b"---\nsecond\n");

    let output = Command::new(env!("CARGO_BIN_EXE_yaml-rt"))
        .args([
            "replace",
            "--query",
            "$.missing",
            "--doc",
            "1",
            "--value",
            "true",
            "--in-place",
            input.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        fs::read(&input).unwrap(),
        b"---\nusers: [{name: first, active: false}]\n---\nusers: [{name: second, active: false}]\n"
    );

    let output = Command::new(env!("CARGO_BIN_EXE_yaml-rt"))
        .args([
            "replace",
            "--query",
            "$.users[*].active",
            "--doc",
            "1",
            "--value",
            "true",
            "--in-place",
            input.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    assert!(output.stdout.is_empty());
    assert_eq!(
        fs::read(&input).unwrap(),
        b"---\nusers: [{name: first, active: false}]\n---\nusers: [{name: second, active: true}]\n"
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn stdin_mutation_and_failed_test_have_expected_streams() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_yaml-rt"))
        .args(["add", "/port", "--value", "8080", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"host: localhost\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    assert_eq!(output.stdout, b"host: localhost\nport: 8080\n");

    let directory = temp_dir();
    let input = directory.join("document.yaml");
    fs::write(&input, "port: 8080\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_yaml-rt"))
        .args(["test", "/port", "--value", "9090", input.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("test failed"));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn remove_preserves_compact_sequence_mapping_items() {
    let directory = temp_dir();
    let input = directory.join("services.yaml");
    let yaml = "# Production services — comments and style stay put\nservices:\n  - name: api\n    port: 8080 # public endpoint\n    enabled: TRUE\n  - {name: worker, port: 8081, enabled: false}\ndefaults: &defaults\n  retries: 0x3\nmirror: *defaults\n";
    fs::write(&input, yaml).unwrap();

    let first = Command::new(env!("CARGO_BIN_EXE_yaml-rt"))
        .args(["remove", "/services/0/name", input.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(first.status.success(), "{:?}", first.stderr);
    assert!(first.stderr.is_empty());
    assert_eq!(
        first.stdout,
        b"# Production services \xe2\x80\x94 comments and style stay put\nservices:\n  - port: 8080 # public endpoint\n    enabled: TRUE\n  - {name: worker, port: 8081, enabled: false}\ndefaults: &defaults\n  retries: 0x3\nmirror: *defaults\n"
    );

    let last = Command::new(env!("CARGO_BIN_EXE_yaml-rt"))
        .args(["remove", "/services/0/enabled", input.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(last.status.success(), "{:?}", last.stderr);
    assert!(last.stderr.is_empty());
    assert_eq!(
        last.stdout,
        b"# Production services \xe2\x80\x94 comments and style stay put\nservices:\n  - name: api\n    port: 8080 # public endpoint\n  - {name: worker, port: 8081, enabled: false}\ndefaults: &defaults\n  retries: 0x3\nmirror: *defaults\n"
    );

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn remove_preserves_empty_block_collection_types() {
    let directory = temp_dir();
    let input = directory.join("collections.yaml");
    fs::write(
        &input,
        "server:\n  host: localhost\nitems:\n  - only\ntail: keep\n",
    )
    .unwrap();

    let mapping = Command::new(env!("CARGO_BIN_EXE_yaml-rt"))
        .args(["remove", "/server/host", input.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(mapping.status.success(), "{:?}", mapping.stderr);
    assert_eq!(
        mapping.stdout,
        b"server: {}\nitems:\n  - only\ntail: keep\n"
    );

    let sequence = Command::new(env!("CARGO_BIN_EXE_yaml-rt"))
        .args(["remove", "/items/0", input.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(sequence.status.success(), "{:?}", sequence.stderr);
    assert_eq!(
        sequence.stdout,
        b"server:\n  host: localhost\nitems: []\ntail: keep\n"
    );

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn in_place_replaces_only_after_success() {
    let directory = temp_dir();
    let input = directory.join("document.yaml");
    fs::write(&input, "port: 8080\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_yaml-rt"))
        .args([
            "replace",
            "--in-place",
            "/missing",
            "--value",
            "9090",
            input.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(fs::read(&input).unwrap(), b"port: 8080\n");

    let output = Command::new(env!("CARGO_BIN_EXE_yaml-rt"))
        .args([
            "replace",
            "--in-place",
            "/port",
            "--value",
            "9090",
            input.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    assert!(output.stdout.is_empty());
    assert_eq!(fs::read(&input).unwrap(), b"port: 9090\n");
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn rename_key_supports_in_place_recursive_directory_targets() {
    let directory = temp_dir();
    fs::create_dir(directory.join("nested")).unwrap();
    fs::write(directory.join("one.yaml"), "old: one # keep\n").unwrap();
    fs::write(directory.join("nested/two.yml"), "old: two\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_yaml-rt"))
        .args([
            "rename-key",
            "/old",
            "--to",
            "new",
            "--in-place",
            directory.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    assert!(output.stdout.is_empty());
    assert_eq!(
        fs::read(directory.join("one.yaml")).unwrap(),
        b"new: one # keep\n"
    );
    assert_eq!(
        fs::read(directory.join("nested/two.yml")).unwrap(),
        b"new: two\n"
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn patch_file_supports_json_document_selection_and_output() {
    let directory = temp_dir();
    let input = directory.join("document.yaml");
    let patch = directory.join("changes.json");
    let result = directory.join("result.yaml");
    fs::write(&input, "---\nname: first\n---\nname: second # keep\n").unwrap();
    fs::write(
        &patch,
        r#"[{"op":"replace","path":"/name","value":"updated"}]"#,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_yaml-rt"))
        .args([
            "patch",
            "--patch-file",
            patch.to_str().unwrap(),
            "--doc",
            "1",
            "--output",
            result.to_str().unwrap(),
            input.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    assert!(output.stdout.is_empty());
    assert_eq!(
        fs::read(&result).unwrap(),
        b"---\nname: first\n---\nname: updated # keep\n"
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn patch_and_target_stdin_combinations_are_checked() {
    let directory = temp_dir();
    let input = directory.join("document.yaml");
    let patch = directory.join("changes.yaml");
    fs::write(&input, "value: 1\n").unwrap();
    fs::write(&patch, "- {op: replace, path: /value, value: 2}\n").unwrap();

    let mut patch_from_stdin = Command::new(env!("CARGO_BIN_EXE_yaml-rt"))
        .args(["patch", "--patch-file", "-", input.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    patch_from_stdin
        .stdin
        .take()
        .unwrap()
        .write_all(b"- {op: replace, path: /value, value: 3}\n")
        .unwrap();
    let output = patch_from_stdin.wait_with_output().unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    assert_eq!(output.stdout, b"value: 3\n");

    let mut target_from_stdin = Command::new(env!("CARGO_BIN_EXE_yaml-rt"))
        .args(["patch", "--patch-file", patch.to_str().unwrap(), "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    target_from_stdin
        .stdin
        .take()
        .unwrap()
        .write_all(b"value: 1\n")
        .unwrap();
    let output = target_from_stdin.wait_with_output().unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    assert_eq!(output.stdout, b"value: 2\n");

    let output = Command::new(env!("CARGO_BIN_EXE_yaml-rt"))
        .args(["patch", "--patch-file", "-", "-"])
        .stdin(Stdio::piped())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("both read stdin"));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn patch_in_place_is_atomic_across_all_operations() {
    let directory = temp_dir();
    let input = directory.join("document.yaml");
    fs::write(&input, "value: 1 # keep\n").unwrap();

    let failing = "- {op: replace, path: /value, value: 2}\n- {op: test, path: /value, value: 3}\n";
    let output = Command::new(env!("CARGO_BIN_EXE_yaml-rt"))
        .args([
            "patch",
            "--patch",
            failing,
            "--in-place",
            input.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(fs::read(&input).unwrap(), b"value: 1 # keep\n");

    let successful = "- {op: replace, path: /value, value: 2}\n";
    let output = Command::new(env!("CARGO_BIN_EXE_yaml-rt"))
        .args([
            "patch",
            "--patch",
            successful,
            "--in-place",
            input.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    assert!(output.stdout.is_empty());
    assert_eq!(fs::read(&input).unwrap(), b"value: 2 # keep\n");
    fs::remove_dir_all(directory).unwrap();
}
