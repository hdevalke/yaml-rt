//! Command-line querying and editing operations for the `yaml-rt` binary.
//!
//! The binary searches YAML documents with JSONPath and applies JSON Pointer
//! operations while retaining unrelated presentation. [`run`] is public so
//! integrations can supply their own argument and I/O streams.

use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use clap::{Args, Parser, Subcommand, error::ErrorKind};
use yaml_rt_core::{JsonPointer, YamlDoc, YamlFragment};

mod query;

use query::run_query;

const FAILURE: i32 = 1;
const USAGE: i32 = 2;
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Runs the command-line application against supplied streams.
pub fn run<I, T>(
    args: I,
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => {
            let display_only = matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            );
            let write_result = if display_only {
                write!(stdout, "{error}")
            } else {
                write!(stderr, "{error}")
            };
            if write_result.is_err() {
                return FAILURE;
            }
            return if display_only { 0 } else { USAGE };
        }
    };
    match execute(&cli.operation, stdin, stdout) {
        Ok(()) => 0,
        Err(RunError::BrokenPipe) => 0,
        Err(RunError::Message(message)) => {
            let _ = writeln!(stderr, "yaml-rt: {message}");
            FAILURE
        }
    }
}

#[derive(Parser)]
#[command(
    name = "yaml-rt",
    version,
    about = "Query and edit YAML while preserving presentation",
    subcommand_required = true,
    arg_required_else_help = true
)]
struct Cli {
    #[command(subcommand)]
    operation: Operation,
}

#[derive(Subcommand)]
enum Operation {
    /// Search a YAML document with RFC 9535 JSONPath.
    Query(QueryArgs),
    /// Print a selected YAML node.
    Get(ReadArgs),
    /// Add or replace a value.
    Add(ValueMutationArgs),
    /// Remove an existing value.
    Remove(MutationArgs),
    /// Replace an existing value.
    Replace(ValueMutationArgs),
    /// Move an existing value.
    Move(FromMutationArgs),
    /// Copy an existing value.
    Copy(FromMutationArgs),
    /// Test semantic equality at a path.
    Test(ValueArgs),
}

#[derive(Args)]
struct TargetArgs {
    /// Input YAML file; defaults to stdin.
    #[arg(value_name = "FILE")]
    file: Option<PathBuf>,
    /// Zero-based YAML document index.
    #[arg(long, value_name = "INDEX")]
    doc: Option<usize>,
}

#[derive(Args)]
struct PathArgs {
    #[arg(value_name = "PATH", allow_hyphen_values = true)]
    path: String,
    #[command(flatten)]
    target: TargetArgs,
}

#[derive(Args)]
struct FromPathArgs {
    #[arg(value_name = "FROM", allow_hyphen_values = true)]
    from: String,
    #[arg(value_name = "PATH", allow_hyphen_values = true)]
    path: String,
    #[command(flatten)]
    target: TargetArgs,
}

#[derive(Args)]
struct OutputArgs {
    /// Write output to a file.
    #[arg(short, long, value_name = "FILE")]
    output: Option<PathBuf>,
}

#[derive(Args)]
struct MutationOutputArgs {
    #[command(flatten)]
    output: OutputArgs,
    /// Atomically replace the input file.
    #[arg(short, long, conflicts_with = "output")]
    in_place: bool,
}

#[derive(Args)]
#[group(required = true, multiple = false)]
struct ValueSourceArgs {
    /// Complete YAML node.
    #[arg(long, value_name = "YAML", allow_hyphen_values = true)]
    value: Option<String>,
    /// Read the YAML node from a file.
    #[arg(long, value_name = "FILE")]
    value_file: Option<PathBuf>,
}

#[derive(Args)]
struct ReadArgs {
    #[command(flatten)]
    path: PathArgs,
    #[command(flatten)]
    output: OutputArgs,
}

#[derive(Args)]
struct QueryArgs {
    /// RFC 9535 JSONPath query.
    #[arg(value_name = "QUERY")]
    query: String,
    #[command(flatten)]
    target: TargetArgs,
    #[command(flatten)]
    output: OutputArgs,
}

#[derive(Args)]
struct MutationArgs {
    #[command(flatten)]
    path: PathArgs,
    #[command(flatten)]
    output: MutationOutputArgs,
}

#[derive(Args)]
struct FromMutationArgs {
    #[command(flatten)]
    path: FromPathArgs,
    #[command(flatten)]
    output: MutationOutputArgs,
}

#[derive(Args)]
struct ValueArgs {
    #[command(flatten)]
    path: PathArgs,
    #[command(flatten)]
    value: ValueSourceArgs,
}

#[derive(Args)]
struct ValueMutationArgs {
    #[command(flatten)]
    value: ValueArgs,
    #[command(flatten)]
    output: MutationOutputArgs,
}

fn execute(
    operation: &Operation,
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
) -> Result<(), RunError> {
    let target = operation.target();
    let input_path = target.file.as_deref();
    let target_uses_stdin = input_path.is_none_or(|path| path == Path::new("-"));
    let input = read_target(input_path, stdin)?;
    let mut doc = YamlDoc::parse_owned(input).map_err(RunError::display)?;
    let document = select_document(&doc, target.doc)?;

    if let Operation::Query(arguments) = operation {
        let output = run_query(&doc, document, &arguments.query).map_err(RunError::display)?;
        return write_result(
            output.as_bytes(),
            arguments.output.output.as_deref(),
            input_path,
            stdout,
        );
    }

    let path = JsonPointer::parse(operation.path()).map_err(RunError::display)?;
    let from = operation
        .from()
        .map(JsonPointer::parse)
        .transpose()
        .map_err(RunError::display)?;
    let value = read_value(operation.value(), target_uses_stdin, stdin)?;

    match operation {
        Operation::Query(_) => unreachable!("query returned before pointer operations"),
        Operation::Get(arguments) => {
            let node = doc
                .resolve_pointer(document, &path)
                .map_err(RunError::display)?;
            let output = doc.extract_node(node).map_err(RunError::display)?;
            write_result(
                output.as_bytes(),
                arguments.output.output.as_deref(),
                input_path,
                stdout,
            )
        }
        Operation::Test(_) => {
            let equal = doc
                .test_at(
                    document,
                    &path,
                    value.as_ref().expect("Clap requires a value"),
                )
                .map_err(RunError::display)?;
            if equal {
                Ok(())
            } else {
                Err(RunError::message(format!(
                    "test failed at {:?}: values are not semantically equal",
                    path.as_str()
                )))
            }
        }
        Operation::Add(arguments) => {
            doc.add_at(
                document,
                &path,
                value.as_ref().expect("Clap requires a value"),
            )
            .map_err(RunError::display)?;
            write_mutation(&doc, &arguments.output, input_path, stdout)
        }
        Operation::Remove(arguments) => {
            doc.remove_at(document, &path).map_err(RunError::display)?;
            write_mutation(&doc, &arguments.output, input_path, stdout)
        }
        Operation::Replace(arguments) => {
            doc.replace_at(
                document,
                &path,
                value.as_ref().expect("Clap requires a value"),
            )
            .map_err(RunError::display)?;
            write_mutation(&doc, &arguments.output, input_path, stdout)
        }
        Operation::Move(arguments) => {
            doc.move_at(document, from.as_ref().expect("Clap requires from"), &path)
                .map_err(RunError::display)?;
            write_mutation(&doc, &arguments.output, input_path, stdout)
        }
        Operation::Copy(arguments) => {
            doc.copy_at(document, from.as_ref().expect("Clap requires from"), &path)
                .map_err(RunError::display)?;
            write_mutation(&doc, &arguments.output, input_path, stdout)
        }
    }
}

impl Operation {
    fn target(&self) -> &TargetArgs {
        match self {
            Self::Query(args) => &args.target,
            Self::Get(args) => &args.path.target,
            Self::Add(args) | Self::Replace(args) => &args.value.path.target,
            Self::Remove(args) => &args.path.target,
            Self::Move(args) | Self::Copy(args) => &args.path.target,
            Self::Test(args) => &args.path.target,
        }
    }

    fn path(&self) -> &str {
        match self {
            Self::Query(_) => unreachable!("query does not use a JSON Pointer argument"),
            Self::Get(args) => &args.path.path,
            Self::Add(args) | Self::Replace(args) => &args.value.path.path,
            Self::Remove(args) => &args.path.path,
            Self::Move(args) | Self::Copy(args) => &args.path.path,
            Self::Test(args) => &args.path.path,
        }
    }

    fn from(&self) -> Option<&str> {
        match self {
            Self::Move(args) | Self::Copy(args) => Some(&args.path.from),
            _ => None,
        }
    }

    fn value(&self) -> Option<&ValueSourceArgs> {
        match self {
            Self::Add(args) | Self::Replace(args) => Some(&args.value.value),
            Self::Test(args) => Some(&args.value),
            _ => None,
        }
    }
}

fn read_target(path: Option<&Path>, stdin: &mut dyn Read) -> Result<String, RunError> {
    match path {
        None => read_stream(stdin, "stdin"),
        Some(path) if path == Path::new("-") => read_stream(stdin, "stdin"),
        Some(path) => fs::read_to_string(path)
            .map_err(|error| RunError::message(format!("cannot read {}: {error}", path.display()))),
    }
}

fn read_value(
    arguments: Option<&ValueSourceArgs>,
    target_uses_stdin: bool,
    stdin: &mut dyn Read,
) -> Result<Option<YamlFragment>, RunError> {
    let input = if let Some(value) = arguments.and_then(|arguments| arguments.value.as_ref()) {
        Some(value.clone())
    } else if let Some(path) = arguments.and_then(|arguments| arguments.value_file.as_deref()) {
        if path == Path::new("-") {
            if target_uses_stdin {
                return Err(RunError::message(
                    "target YAML and --value-file cannot both read stdin",
                ));
            }
            Some(read_stream(stdin, "value stdin")?)
        } else {
            Some(fs::read_to_string(path).map_err(|error| {
                RunError::message(format!(
                    "cannot read value file {}: {error}",
                    path.display()
                ))
            })?)
        }
    } else {
        None
    };
    input
        .map(YamlFragment::parse_owned)
        .transpose()
        .map_err(RunError::display)
}

fn read_stream(stream: &mut dyn Read, name: &str) -> Result<String, RunError> {
    let mut input = String::new();
    stream
        .read_to_string(&mut input)
        .map_err(|error| RunError::message(format!("cannot read {name}: {error}")))?;
    Ok(input)
}

fn select_document(doc: &YamlDoc, selected: Option<usize>) -> Result<usize, RunError> {
    let count = doc.document_count();
    match selected {
        Some(index) if index < count => Ok(index),
        Some(index) => Err(RunError::message(format!(
            "document index {index} is out of range for {count} documents"
        ))),
        None if count == 1 => Ok(0),
        None if count == 0 => Err(RunError::message("YAML stream contains no documents")),
        None => Err(RunError::message(format!(
            "YAML stream contains {count} documents; select one with --doc"
        ))),
    }
}

fn write_mutation(
    doc: &YamlDoc,
    arguments: &MutationOutputArgs,
    input: Option<&Path>,
    stdout: &mut dyn Write,
) -> Result<(), RunError> {
    if arguments.in_place {
        let input = input
            .filter(|path| *path != Path::new("-"))
            .ok_or_else(|| RunError::message("--in-place requires a real input filename"))?;
        atomic_replace(input, doc.as_source().as_bytes())
    } else {
        write_result(
            doc.as_source().as_bytes(),
            arguments.output.output.as_deref(),
            input,
            stdout,
        )
    }
}

fn write_result(
    bytes: &[u8],
    output: Option<&Path>,
    input: Option<&Path>,
    stdout: &mut dyn Write,
) -> Result<(), RunError> {
    if let Some(output) = output {
        if input.is_some_and(|input| paths_equivalent(input, output)) {
            return Err(RunError::message(
                "--output must not name the input file; use --in-place",
            ));
        }
        fs::write(output, bytes).map_err(|error| {
            RunError::message(format!("cannot write {}: {error}", output.display()))
        })
    } else {
        stdout.write_all(bytes).map_err(RunError::io)?;
        stdout.flush().map_err(RunError::io)
    }
}

fn paths_equivalent(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => absolute_path(left).ok() == absolute_path(right).ok(),
    }
}

fn absolute_path(path: &Path) -> io::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_owned())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn atomic_replace(path: &Path, bytes: &[u8]) -> Result<(), RunError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        RunError::message(format!("cannot inspect {}: {error}", path.display()))
    })?;
    if metadata.file_type().is_symlink() {
        return Err(RunError::message(
            "--in-place refuses to replace a symbolic link",
        ));
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .ok_or_else(|| RunError::message("input path has no filename"))?;
    let (temporary, mut file) = create_sibling_temp(parent, file_name)?;
    let mut guard = TempGuard {
        path: temporary.clone(),
        armed: true,
    };
    file.set_permissions(metadata.permissions())
        .map_err(|error| {
            RunError::message(format!(
                "cannot preserve permissions for {}: {error}",
                path.display()
            ))
        })?;
    file.write_all(bytes).map_err(RunError::io)?;
    file.flush().map_err(RunError::io)?;
    file.sync_all().map_err(RunError::io)?;
    drop(file);
    fs::rename(&temporary, path).map_err(|error| {
        RunError::message(format!(
            "cannot atomically replace {}: {error}",
            path.display()
        ))
    })?;
    guard.armed = false;
    Ok(())
}

fn create_sibling_temp(parent: &Path, file_name: &OsStr) -> Result<(PathBuf, File), RunError> {
    for _ in 0..100 {
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut name = OsString::from(".");
        name.push(file_name);
        name.push(format!(".yaml-rt-{}-{counter}.tmp", std::process::id()));
        let path = parent.join(name);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(RunError::message(format!(
                    "cannot create temporary file in {}: {error}",
                    parent.display()
                )));
            }
        }
    }
    Err(RunError::message(
        "could not allocate a unique temporary filename",
    ))
}

struct TempGuard {
    path: PathBuf,
    armed: bool,
}

impl Drop for TempGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

enum RunError {
    BrokenPipe,
    Message(String),
}

impl RunError {
    fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }

    fn display(error: impl std::fmt::Display) -> Self {
        Self::Message(error.to_string())
    }

    fn io(error: io::Error) -> Self {
        if error.kind() == io::ErrorKind::BrokenPipe {
            Self::BrokenPipe
        } else {
            Self::Message(error.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invoke(args: &[&str], input: &str) -> (i32, String, String) {
        let mut stdin = input.as_bytes();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let status = run(args, &mut stdin, &mut stdout, &mut stderr);
        (
            status,
            String::from_utf8(stdout).unwrap(),
            String::from_utf8(stderr).unwrap(),
        )
    }

    #[test]
    fn get_and_replace_work_with_stdin() {
        let (status, stdout, stderr) = invoke(
            &["yaml-rt", "get", "/server/host"],
            "server:\n  host: localhost\n",
        );
        assert_eq!(status, 0, "{stderr}");
        assert_eq!(stdout, "localhost");

        let (status, stdout, stderr) = invoke(
            &[
                "yaml-rt",
                "replace",
                "/server/host",
                "--value",
                "example.com",
            ],
            "server:\n  host: localhost\n",
        );
        assert_eq!(status, 0, "{stderr}");
        assert_eq!(stdout, "server:\n  host: example.com\n");
    }

    #[test]
    fn query_works_with_stdin_and_no_matches_succeed() {
        let input = "users:\n  - {name: Ada, active: true}\n  - {name: Linus, active: false}\n";
        let (status, stdout, stderr) = invoke(
            &["yaml-rt", "query", "$.users[?@.active == true].name"],
            input,
        );
        assert_eq!(status, 0, "{stderr}");
        assert_eq!(stdout, "\"/users/0/name\": \"Ada\"\n");

        let (status, stdout, stderr) = invoke(&["yaml-rt", "query", "$.missing"], input);
        assert_eq!(status, 0, "{stderr}");
        assert!(stdout.is_empty());
    }

    #[test]
    fn test_failure_has_no_stdout() {
        let (status, stdout, stderr) =
            invoke(&["yaml-rt", "test", "/value", "--value", "2"], "value: 1\n");
        assert_eq!(status, FAILURE);
        assert!(stdout.is_empty());
        assert!(stderr.contains("test failed"));
    }

    #[test]
    fn multiple_documents_require_selection() {
        let (status, _, stderr) = invoke(&["yaml-rt", "get", ""], "--- one\n--- two\n");
        assert_eq!(status, FAILURE);
        assert!(stderr.contains("--doc"));
    }

    #[test]
    fn derive_arguments_enforce_value_and_output_conflicts() {
        let (status, stdout, stderr) = invoke(&["yaml-rt", "replace", "/value"], "value: 1\n");
        assert_eq!(status, USAGE);
        assert!(stdout.is_empty());
        assert!(stderr.contains("--value"));

        let (status, stdout, stderr) = invoke(
            &[
                "yaml-rt",
                "replace",
                "/value",
                "--value",
                "1",
                "--value-file",
                "value.yaml",
            ],
            "value: 1\n",
        );
        assert_eq!(status, USAGE);
        assert!(stdout.is_empty());
        assert!(stderr.contains("cannot be used with"));

        let (status, stdout, stderr) = invoke(
            &[
                "yaml-rt",
                "remove",
                "/value",
                "--output",
                "out.yaml",
                "--in-place",
            ],
            "value: 1\n",
        );
        assert_eq!(status, USAGE);
        assert!(stdout.is_empty());
        assert!(stderr.contains("cannot be used with"));
    }

    #[test]
    fn hyphen_prefixed_inline_yaml_is_accepted() {
        let (status, stdout, stderr) = invoke(&["yaml-rt", "get", "-invalid"], "value: old\n");
        assert_eq!(status, FAILURE);
        assert!(stdout.is_empty());
        assert!(stderr.contains("JSON Pointer"));

        let (status, stdout, stderr) = invoke(
            &["yaml-rt", "replace", "/value", "--value", "-1"],
            "value: old\n",
        );
        assert_eq!(status, 0, "{stderr}");
        assert_eq!(stdout, "value: -1\n");
    }
}
