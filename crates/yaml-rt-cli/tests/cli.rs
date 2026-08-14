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
fn stdin_mutation_and_failed_test_have_expected_streams() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_yaml-rt"))
        .args(["add", "/port", "--value", "8080"])
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
        .args(["patch", "--patch-file", patch.to_str().unwrap()])
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
        .args(["patch", "--patch-file", "-"])
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
