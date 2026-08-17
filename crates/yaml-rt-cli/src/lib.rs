//! Command-line querying and editing operations for the `yaml-rt` binary.
//!
//! The binary searches YAML documents with `JSONPath`, applies JSON Pointer
//! operations, and executes transactional YAML or JSON patch documents while
//! retaining unrelated presentation. File targets can also be directories,
//! which are searched recursively for YAML files; an omitted target searches
//! the current directory, while `-` reads standard input. [`run`] is public so
//! integrations can supply their own argument and I/O streams.

use std::cmp::Ordering as CmpOrdering;
use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use clap::{Args, CommandFactory, Parser, Subcommand, error::ErrorKind};
use yaml_rt_core::{JsonPointer, YamlDoc, YamlFragment, YamlPatch};
use yaml_rt_rfc9535::{JsonPath, QueryMatches};

mod query;

use query::{query_matches, run_query};

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
    if let Err(message) = cli.operation.validate() {
        let error = Cli::command().error(ErrorKind::ArgumentConflict, message);
        if write!(stderr, "{error}").is_err() {
            return FAILURE;
        }
        return USAGE;
    }
    match execute(&cli.operation, stdin, stdout) {
        Ok(()) | Err(RunError::BrokenPipe) => 0,
        Err(RunError::Usage(message)) => {
            let error = Cli::command().error(ErrorKind::ArgumentConflict, message);
            if write!(stderr, "{error}").is_err() {
                return FAILURE;
            }
            USAGE
        }
        Err(RunError::Batch {
            diagnostics,
            summary,
        }) => {
            for diagnostic in diagnostics {
                let _ = writeln!(stderr, "yaml-rt: {diagnostic}");
            }
            let _ = writeln!(stderr, "yaml-rt: {summary}");
            FAILURE
        }
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
    /// Search a YAML document with RFC 9535 `JSONPath`.
    Query(QueryArgs),
    /// Print a selected YAML node.
    Get(ReadArgs),
    /// Add or replace a value.
    Add(ValueMutationArgs),
    /// Remove an existing value.
    Remove(MutationArgs),
    /// Replace an existing value.
    Replace(ValueMutationArgs),
    /// Rename one or more mapping keys.
    RenameKey(RenameKeyArgs),
    /// Move an existing value.
    Move(FromMutationArgs),
    /// Copy an existing value.
    Copy(FromMutationArgs),
    /// Test semantic equality at a path.
    Test(ValueArgs),
    /// Apply a transactional YAML or JSON patch document.
    Patch(PatchArgs),
}

#[derive(Args)]
struct TargetArgs {
    /// Input YAML file or directory; defaults to the current directory. Use - for stdin.
    #[arg(value_name = "FILE")]
    file: Option<PathBuf>,
    /// Zero-based YAML document index.
    #[arg(long, value_name = "INDEX")]
    doc: Option<usize>,
}

#[derive(Args)]
struct PathArgs {
    /// JSON Pointer, or the input file when `--query` is used.
    #[arg(
        value_name = "PATH_OR_FILE",
        allow_hyphen_values = true,
        required_unless_present = "query"
    )]
    path_or_file: Option<String>,
    /// Select operation targets with an RFC 9535 `JSONPath` query.
    #[arg(long, value_name = "QUERY")]
    query: Option<String>,
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
#[group(required = true, multiple = false)]
struct PatchSourceArgs {
    /// YAML or JSON patch document.
    #[arg(long, value_name = "YAML", allow_hyphen_values = true)]
    patch: Option<String>,
    /// Read the YAML or JSON patch document from a file.
    #[arg(long, value_name = "FILE")]
    patch_file: Option<PathBuf>,
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
    /// RFC 9535 `JSONPath` query.
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

#[derive(Args)]
struct RenameKeyArgs {
    #[command(flatten)]
    path: PathArgs,
    /// Decoded destination key name.
    #[arg(long, value_name = "KEY")]
    to: String,
    #[command(flatten)]
    output: MutationOutputArgs,
}

#[derive(Args)]
struct PatchArgs {
    #[command(flatten)]
    target: TargetArgs,
    #[command(flatten)]
    source: PatchSourceArgs,
    #[command(flatten)]
    output: MutationOutputArgs,
}

fn execute(
    operation: &Operation,
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
) -> Result<(), RunError> {
    let targets = resolve_targets(operation.input_path())?;
    if matches!(targets, InputTargets::Batch { .. })
        && operation
            .mutation_output()
            .is_some_and(|output| !output.in_place)
    {
        return Err(RunError::usage(
            "directory targets require --in-place for mutations",
        ));
    }
    let target_uses_stdin = matches!(targets, InputTargets::Stdin);
    if matches!(operation, Operation::Patch(arguments) if arguments.source.patch_file.as_deref() == Some(Path::new("-")))
        && target_uses_stdin
    {
        return Err(RunError::message(
            "target YAML and --patch-file cannot both read stdin",
        ));
    }
    let prepared = prepare_operation(operation, target_uses_stdin, stdin)?;
    match targets {
        InputTargets::Stdin => {
            let input = read_stream(stdin, "stdin")?;
            execute_one(operation, &prepared, None, input, stdout, false)
        }
        InputTargets::File(path) => {
            let input = read_target(&path)?;
            execute_one(operation, &prepared, Some(&path), input, stdout, false)
        }
        InputTargets::Batch {
            files,
            discovery_failures,
        } => execute_batch(operation, &prepared, &files, discovery_failures, stdout),
    }
}

fn execute_one(
    operation: &Operation,
    prepared: &PreparedOperation,
    input_path: Option<&Path>,
    input: String,
    stdout: &mut dyn Write,
    batch_capture: bool,
) -> Result<(), RunError> {
    let target = operation.target();
    let mut doc = YamlDoc::parse_owned(input).map_err(RunError::display)?;
    let document = select_document(&doc, target.doc)?;

    if let Operation::Query(arguments) = operation {
        let output = run_query(
            &doc,
            document,
            prepared
                .query
                .as_ref()
                .expect("query operation is prepared"),
        )
        .map_err(RunError::display)?;
        return write_result(
            output.as_bytes(),
            if batch_capture {
                None
            } else {
                arguments.output.output.as_deref()
            },
            input_path,
            stdout,
        );
    }

    if let Operation::Patch(arguments) = operation {
        doc.apply_patch(
            document,
            prepared
                .patch
                .as_ref()
                .expect("patch operation is prepared"),
        )
        .map_err(RunError::display)?;
        return write_mutation(&doc, &arguments.output, input_path, stdout);
    }

    if operation.selection_query().is_some() {
        let matches = query_matches(
            &doc,
            document,
            prepared
                .query
                .as_ref()
                .expect("query-targeted operation is prepared"),
        )
        .map_err(RunError::display)?;
        let mut output = CommandOutput {
            input_path,
            stdout,
            batch_capture,
        };
        return execute_query_targeted(
            operation,
            &mut doc,
            document,
            &matches,
            prepared.value.as_ref(),
            &mut output,
        );
    }

    let path = prepared
        .path
        .as_ref()
        .expect("pointer operation is prepared");
    let from = prepared.from.as_ref();
    match operation {
        Operation::Query(_) => unreachable!("query returned before pointer operations"),
        Operation::Get(arguments) => {
            let node = doc
                .resolve_pointer(document, path)
                .map_err(RunError::display)?;
            let output = doc.extract_node(node).map_err(RunError::display)?;
            write_result(
                output.as_bytes(),
                if batch_capture {
                    None
                } else {
                    arguments.output.output.as_deref()
                },
                input_path,
                stdout,
            )
        }
        Operation::Test(_) => {
            let equal = doc
                .test_at(
                    document,
                    path,
                    prepared.value.as_ref().expect("Clap requires a value"),
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
                path,
                prepared.value.as_ref().expect("Clap requires a value"),
            )
            .map_err(RunError::display)?;
            write_mutation(&doc, &arguments.output, input_path, stdout)
        }
        Operation::Remove(arguments) => {
            doc.remove_at(document, path).map_err(RunError::display)?;
            write_mutation(&doc, &arguments.output, input_path, stdout)
        }
        Operation::Replace(arguments) => {
            doc.replace_at(
                document,
                path,
                prepared.value.as_ref().expect("Clap requires a value"),
            )
            .map_err(RunError::display)?;
            write_mutation(&doc, &arguments.output, input_path, stdout)
        }
        Operation::RenameKey(arguments) => {
            doc.rename_key_at(document, path, &arguments.to)
                .map_err(RunError::display)?;
            write_mutation(&doc, &arguments.output, input_path, stdout)
        }
        Operation::Move(arguments) => {
            doc.move_at(document, from.expect("Clap requires from"), path)
                .map_err(RunError::display)?;
            write_mutation(&doc, &arguments.output, input_path, stdout)
        }
        Operation::Copy(arguments) => {
            doc.copy_at(document, from.expect("Clap requires from"), path)
                .map_err(RunError::display)?;
            write_mutation(&doc, &arguments.output, input_path, stdout)
        }
        Operation::Patch(_) => unreachable!("patch returned before pointer operations"),
    }
}

struct PreparedOperation {
    path: Option<JsonPointer>,
    from: Option<JsonPointer>,
    query: Option<JsonPath>,
    value: Option<YamlFragment>,
    patch: Option<YamlPatch>,
}

fn prepare_operation(
    operation: &Operation,
    target_uses_stdin: bool,
    stdin: &mut dyn Read,
) -> Result<PreparedOperation, RunError> {
    let query = operation
        .query_source()
        .map(JsonPath::parse)
        .transpose()
        .map_err(RunError::display)?;
    let path = if query.is_none() && !matches!(operation, Operation::Patch(_)) {
        Some(JsonPointer::parse(operation.path()).map_err(RunError::display)?)
    } else {
        None
    };
    let from = operation
        .from()
        .map(JsonPointer::parse)
        .transpose()
        .map_err(RunError::display)?;
    let value = read_value(operation.value(), target_uses_stdin, stdin)?;
    let patch = match operation {
        Operation::Patch(arguments) => Some(read_patch(&arguments.source, stdin)?),
        _ => None,
    };
    Ok(PreparedOperation {
        path,
        from,
        query,
        value,
        patch,
    })
}

enum InputTargets {
    Stdin,
    File(PathBuf),
    Batch {
        files: Vec<BatchTarget>,
        discovery_failures: Vec<DiscoveryFailure>,
    },
}

struct BatchTarget {
    path: PathBuf,
    relative: PathBuf,
}

struct DiscoveryFailure {
    relative: PathBuf,
    message: String,
}

fn resolve_targets(path: Option<&Path>) -> Result<InputTargets, RunError> {
    let path = match path {
        None => std::env::current_dir().map_err(|error| {
            RunError::message(format!("cannot determine current directory: {error}"))
        })?,
        Some(path) if path == Path::new("-") => return Ok(InputTargets::Stdin),
        Some(path) => path.to_owned(),
    };
    if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_dir()) {
        let (files, discovery_failures) = discover_yaml_files(&path);
        Ok(InputTargets::Batch {
            files,
            discovery_failures,
        })
    } else {
        Ok(InputTargets::File(path))
    }
}

fn discover_yaml_files(root: &Path) -> (Vec<BatchTarget>, Vec<DiscoveryFailure>) {
    let mut files = Vec::new();
    let mut failures = Vec::new();
    discover_directory(root, root, &mut files, &mut failures);
    files.sort_by(|left, right| left.relative.cmp(&right.relative));
    failures.sort_by(|left, right| left.relative.cmp(&right.relative));
    (files, failures)
}

fn discover_directory(
    root: &Path,
    directory: &Path,
    files: &mut Vec<BatchTarget>,
    failures: &mut Vec<DiscoveryFailure>,
) {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => {
            failures.push(DiscoveryFailure {
                relative: relative_to(root, directory),
                message: format!("cannot read directory: {error}"),
            });
            return;
        }
    };
    let mut entries = entries
        .filter_map(|entry| match entry {
            Ok(entry) => Some(entry),
            Err(error) => {
                failures.push(DiscoveryFailure {
                    relative: relative_to(root, directory),
                    message: format!("cannot read directory entry: {error}"),
                });
                None
            }
        })
        .collect::<Vec<_>>();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                failures.push(DiscoveryFailure {
                    relative: relative_to(root, &path),
                    message: format!("cannot inspect path: {error}"),
                });
                continue;
            }
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            discover_directory(root, &path, files, failures);
        } else if file_type.is_file() && has_yaml_extension(&path) {
            files.push(BatchTarget {
                relative: relative_to(root, &path),
                path,
            });
        }
    }
}

fn relative_to(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root)
        .ok()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .to_owned()
}

fn has_yaml_extension(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("yaml") || extension.eq_ignore_ascii_case("yml")
        })
}

fn execute_batch(
    operation: &Operation,
    prepared: &PreparedOperation,
    files: &[BatchTarget],
    discovery_failures: Vec<DiscoveryFailure>,
    stdout: &mut dyn Write,
) -> Result<(), RunError> {
    if let Some(output) = operation.read_output()
        && let Some(input) = files
            .iter()
            .find(|input| paths_equivalent(&input.path, output))
    {
        return Err(RunError::message(format!(
            "--output must not name input file {}",
            render_batch_path(&input.relative)
        )));
    }

    let mut diagnostics = discovery_failures
        .iter()
        .map(|failure| {
            format!(
                "{}: {}",
                render_batch_path(&failure.relative),
                failure.message
            )
        })
        .collect::<Vec<_>>();
    let mut succeeded = 0;
    let mut failed = 0;
    let mut combined_output = Vec::new();
    for input in files {
        let source = match read_target(&input.path) {
            Ok(source) => source,
            Err(RunError::Message(message)) => {
                diagnostics.push(format!("{}: {message}", render_batch_path(&input.relative)));
                failed += 1;
                continue;
            }
            Err(error) => return Err(error),
        };
        let mut result = Vec::new();
        match execute_one(
            operation,
            prepared,
            Some(&input.path),
            source,
            &mut result,
            true,
        ) {
            Ok(()) => {
                succeeded += 1;
                if operation.should_emit_batch_result(&result) {
                    append_batch_result(&mut combined_output, &input.relative, &result);
                }
            }
            Err(RunError::Message(message)) => {
                diagnostics.push(format!("{}: {message}", render_batch_path(&input.relative)));
                failed += 1;
            }
            Err(error) => return Err(error),
        }
    }

    if operation.has_read_output() {
        write_result(&combined_output, operation.read_output(), None, stdout)?;
    }
    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(RunError::Batch {
            diagnostics,
            summary: format!(
                "processed {} YAML files: {succeeded} succeeded, {failed} failed; {} traversal errors",
                files.len(),
                discovery_failures.len()
            ),
        })
    }
}

fn append_batch_result(output: &mut Vec<u8>, path: &Path, result: &[u8]) {
    if !output.is_empty() {
        output.push(b'\n');
    }
    writeln!(output, "==> {} <==", render_batch_path(path)).expect("writing to a Vec cannot fail");
    output.extend_from_slice(result);
    if !result.is_empty() && !result.ends_with(b"\n") {
        output.push(b'\n');
    }
}

fn render_batch_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

impl Operation {
    fn validate(&self) -> Result<(), String> {
        let path = match self {
            Self::Get(args) => Some(&args.path),
            Self::Add(args) | Self::Replace(args) => Some(&args.value.path),
            Self::RenameKey(args) => Some(&args.path),
            Self::Remove(args) => Some(&args.path),
            Self::Test(args) => Some(&args.path),
            _ => None,
        };
        if let Some(path) = path {
            path.validate()?;
        }
        Ok(())
    }

    fn target(&self) -> &TargetArgs {
        match self {
            Self::Query(args) => &args.target,
            Self::Get(args) => &args.path.target,
            Self::Add(args) | Self::Replace(args) => &args.value.path.target,
            Self::RenameKey(args) => &args.path.target,
            Self::Remove(args) => &args.path.target,
            Self::Move(args) | Self::Copy(args) => &args.path.target,
            Self::Test(args) => &args.path.target,
            Self::Patch(args) => &args.target,
        }
    }

    fn path(&self) -> &str {
        match self {
            Self::Query(_) | Self::Patch(_) => {
                unreachable!("operation does not use a JSON Pointer argument")
            }
            Self::Get(args) => args.path.pointer(),
            Self::Add(args) | Self::Replace(args) => args.value.path.pointer(),
            Self::RenameKey(args) => args.path.pointer(),
            Self::Remove(args) => args.path.pointer(),
            Self::Move(args) | Self::Copy(args) => &args.path.path,
            Self::Test(args) => args.path.pointer(),
        }
    }

    fn selection_query(&self) -> Option<&str> {
        match self {
            Self::Get(args) => args.path.query.as_deref(),
            Self::Add(args) | Self::Replace(args) => args.value.path.query.as_deref(),
            Self::RenameKey(args) => args.path.query.as_deref(),
            Self::Remove(args) => args.path.query.as_deref(),
            Self::Test(args) => args.path.query.as_deref(),
            _ => None,
        }
    }

    fn query_source(&self) -> Option<&str> {
        match self {
            Self::Query(args) => Some(&args.query),
            _ => self.selection_query(),
        }
    }

    fn mutation_output(&self) -> Option<&MutationOutputArgs> {
        match self {
            Self::Add(args) | Self::Replace(args) => Some(&args.output),
            Self::RenameKey(args) => Some(&args.output),
            Self::Remove(args) => Some(&args.output),
            Self::Move(args) | Self::Copy(args) => Some(&args.output),
            Self::Patch(args) => Some(&args.output),
            Self::Query(_) | Self::Get(_) | Self::Test(_) => None,
        }
    }

    fn read_output(&self) -> Option<&Path> {
        match self {
            Self::Query(args) => args.output.output.as_deref(),
            Self::Get(args) => args.output.output.as_deref(),
            _ => None,
        }
    }

    fn has_read_output(&self) -> bool {
        matches!(self, Self::Query(_) | Self::Get(_))
    }

    fn should_emit_batch_result(&self, result: &[u8]) -> bool {
        match self {
            Self::Query(_) => !result.is_empty(),
            Self::Get(args) if args.path.query.is_some() => !result.is_empty(),
            Self::Get(_) => true,
            _ => false,
        }
    }

    fn input_path(&self) -> Option<&Path> {
        match self {
            Self::Get(args) => args.path.input_path(),
            Self::Add(args) | Self::Replace(args) => args.value.path.input_path(),
            Self::RenameKey(args) => args.path.input_path(),
            Self::Remove(args) => args.path.input_path(),
            Self::Test(args) => args.path.input_path(),
            _ => self.target().file.as_deref(),
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

impl PathArgs {
    fn validate(&self) -> Result<(), String> {
        if self.query.is_some() && self.target.file.is_some() {
            return Err(
                "a JSONPath-targeted command accepts at most one positional FILE argument"
                    .to_owned(),
            );
        }
        Ok(())
    }

    fn pointer(&self) -> &str {
        self.path_or_file
            .as_deref()
            .expect("Clap requires a pointer when --query is absent")
    }

    fn input_path(&self) -> Option<&Path> {
        if self.query.is_some() {
            self.path_or_file.as_deref().map(Path::new)
        } else {
            self.target.file.as_deref()
        }
    }
}

struct CommandOutput<'a> {
    input_path: Option<&'a Path>,
    stdout: &'a mut dyn Write,
    batch_capture: bool,
}

fn execute_query_targeted(
    operation: &Operation,
    doc: &mut YamlDoc,
    document: usize,
    matches: &QueryMatches,
    value: Option<&YamlFragment>,
    output: &mut CommandOutput<'_>,
) -> Result<(), RunError> {
    match operation {
        Operation::Get(arguments) => {
            let rendered = render_yaml_stream(doc, matches)?;
            write_result(
                rendered.as_bytes(),
                if output.batch_capture {
                    None
                } else {
                    arguments.output.output.as_deref()
                },
                output.input_path,
                output.stdout,
            )
        }
        Operation::Test(_) => test_query_matches(
            doc,
            document,
            matches,
            value.expect("Clap requires a value"),
        ),
        Operation::Add(arguments) => {
            apply_query_mutation(
                doc,
                document,
                matches,
                QueryMutation::Add(value.expect("Clap requires a value")),
            )?;
            write_mutation(doc, &arguments.output, output.input_path, output.stdout)
        }
        Operation::Remove(arguments) => {
            apply_query_mutation(doc, document, matches, QueryMutation::Remove)?;
            write_mutation(doc, &arguments.output, output.input_path, output.stdout)
        }
        Operation::Replace(arguments) => {
            apply_query_mutation(
                doc,
                document,
                matches,
                QueryMutation::Replace(value.expect("Clap requires a value")),
            )?;
            write_mutation(doc, &arguments.output, output.input_path, output.stdout)
        }
        Operation::RenameKey(arguments) => {
            if matches.is_empty() {
                return Err(RunError::message("query matched no nodes"));
            }
            let pointers = matches
                .iter()
                .map(|matched| matched.pointer().clone())
                .collect::<Vec<_>>();
            doc.rename_keys_at(document, &pointers, &arguments.to)
                .map_err(RunError::display)?;
            write_mutation(doc, &arguments.output, output.input_path, output.stdout)
        }
        _ => unreachable!("only single-path commands accept --query"),
    }
}

fn render_yaml_stream(doc: &YamlDoc, matches: &QueryMatches) -> Result<String, RunError> {
    let mut output = String::new();
    for matched in matches {
        output.push_str("---\n");
        if let Some(node) = matched.node() {
            let fragment = doc.extract_node(node).map_err(RunError::display)?;
            output.push_str(&fragment);
            if !fragment.ends_with(['\n', '\r']) {
                output.push('\n');
            }
        }
    }
    Ok(output)
}

enum QueryMutation<'a> {
    Add(&'a YamlFragment),
    Remove,
    Replace(&'a YamlFragment),
}

fn apply_query_mutation(
    doc: &mut YamlDoc,
    document: usize,
    matches: &QueryMatches,
    mutation: QueryMutation<'_>,
) -> Result<(), RunError> {
    if matches.is_empty() {
        return Err(RunError::message("query matched no nodes"));
    }
    let mut targets = normalized_mutation_targets(matches);
    if matches!(mutation, QueryMutation::Remove) {
        targets.sort_by(removal_order);
    }
    let mut work = doc.clone();
    for pointer in &targets {
        match mutation {
            QueryMutation::Add(value) => work.add_at(document, pointer, value),
            QueryMutation::Remove => work.remove_at(document, pointer),
            QueryMutation::Replace(value) => work.replace_at(document, pointer, value),
        }
        .map_err(RunError::display)?;
    }
    *doc = work;
    Ok(())
}

fn normalized_mutation_targets(matches: &QueryMatches) -> Vec<JsonPointer> {
    let mut seen = HashSet::new();
    let unique = matches
        .iter()
        .filter_map(|matched| {
            let pointer = matched.pointer();
            seen.insert(pointer.as_str().to_owned())
                .then(|| pointer.clone())
        })
        .collect::<Vec<_>>();
    unique
        .iter()
        .filter(|pointer| {
            !unique
                .iter()
                .any(|candidate| candidate.is_proper_prefix_of(pointer))
        })
        .cloned()
        .collect()
}

fn removal_order(left: &JsonPointer, right: &JsonPointer) -> CmpOrdering {
    right
        .tokens()
        .len()
        .cmp(&left.tokens().len())
        .then_with(|| {
            for (left, right) in left.tokens().iter().zip(right.tokens()) {
                let order = match (
                    left.as_str().parse::<usize>(),
                    right.as_str().parse::<usize>(),
                ) {
                    (Ok(left), Ok(right)) => right.cmp(&left),
                    _ => right.as_str().cmp(left.as_str()),
                };
                if order != CmpOrdering::Equal {
                    return order;
                }
            }
            CmpOrdering::Equal
        })
}

fn test_query_matches(
    doc: &YamlDoc,
    document: usize,
    matches: &QueryMatches,
    value: &YamlFragment,
) -> Result<(), RunError> {
    if matches.is_empty() {
        return Err(RunError::message("query matched no nodes"));
    }
    for matched in matches {
        let pointer = matched.pointer();
        let equal = doc
            .test_at(document, pointer, value)
            .map_err(RunError::display)?;
        if !equal {
            return Err(RunError::message(format!(
                "test failed at {:?}: values are not semantically equal",
                pointer.as_str()
            )));
        }
    }
    Ok(())
}

fn read_patch(arguments: &PatchSourceArgs, stdin: &mut dyn Read) -> Result<YamlPatch, RunError> {
    let input = if let Some(patch) = &arguments.patch {
        patch.clone()
    } else if let Some(path) = arguments.patch_file.as_deref() {
        if path == Path::new("-") {
            read_stream(stdin, "patch stdin")?
        } else {
            fs::read_to_string(path).map_err(|error| {
                RunError::message(format!(
                    "cannot read patch file {}: {error}",
                    path.display()
                ))
            })?
        }
    } else {
        unreachable!("Clap requires a patch source")
    };
    YamlPatch::parse_owned(input).map_err(RunError::display)
}

fn read_target(path: &Path) -> Result<String, RunError> {
    fs::read_to_string(path)
        .map_err(|error| RunError::message(format!("cannot read {}: {error}", path.display())))
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
        stdout
            .write_all(bytes)
            .map_err(|error| RunError::io(&error))?;
        stdout.flush().map_err(|error| RunError::io(&error))
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
    file.write_all(bytes)
        .map_err(|error| RunError::io(&error))?;
    file.flush().map_err(|error| RunError::io(&error))?;
    file.sync_all().map_err(|error| RunError::io(&error))?;
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
    Usage(String),
    Batch {
        diagnostics: Vec<String>,
        summary: String,
    },
}

impl RunError {
    fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }

    fn usage(message: impl Into<String>) -> Self {
        Self::Usage(message.into())
    }

    fn display(error: impl std::fmt::Display) -> Self {
        Self::Message(error.to_string())
    }

    fn io(error: &io::Error) -> Self {
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
        let mut args = args.to_vec();
        args.push("-");
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
    fn rename_key_works_with_pointer_and_document_selection() {
        let input = "---\nold: first\n---\nold: second # keep\n";
        let (status, stdout, stderr) = invoke(
            &[
                "yaml-rt",
                "rename-key",
                "/old",
                "--to",
                "true",
                "--doc",
                "1",
            ],
            input,
        );
        assert_eq!(status, 0, "{stderr}");
        assert_eq!(stdout, "---\nold: first\n---\n\"true\": second # keep\n");
    }

    #[test]
    fn query_targeted_rename_is_atomic_and_requires_mapping_members() {
        let input = "items: [{old: 1}, {old: 2}]\n";
        let (status, stdout, stderr) = invoke(
            &["yaml-rt", "rename-key", "--query", "$..old", "--to", "new"],
            input,
        );
        assert_eq!(status, 0, "{stderr}");
        assert_eq!(stdout, "items: [{new: 1}, {new: 2}]\n");

        let (status, stdout, stderr) = invoke(
            &["yaml-rt", "rename-key", "--query", "$..*", "--to", "new"],
            input,
        );
        assert_eq!(status, FAILURE);
        assert!(stdout.is_empty());
        assert!(stderr.contains("does not select a mapping member"));

        let (status, stdout, stderr) = invoke(
            &[
                "yaml-rt",
                "rename-key",
                "--query",
                "$.missing",
                "--to",
                "new",
            ],
            input,
        );
        assert_eq!(status, FAILURE);
        assert!(stdout.is_empty());
        assert!(stderr.contains("query matched no nodes"));
    }

    #[test]
    fn query_targeted_rename_rolls_back_collisions() {
        let (status, stdout, stderr) = invoke(
            &[
                "yaml-rt",
                "rename-key",
                "--query",
                "$['a','b']",
                "--to",
                "x",
            ],
            "a: 1\nb: 2\n",
        );
        assert_eq!(status, FAILURE);
        assert!(stdout.is_empty());
        assert!(stderr.contains("duplicate key \"x\""));
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
    fn get_query_emits_a_yaml_document_stream() {
        let input = "users:\n  - {name: Ada}\n  - {name: Linus}\n";
        let (status, stdout, stderr) =
            invoke(&["yaml-rt", "get", "--query", "$.users[*].name"], input);
        assert_eq!(status, 0, "{stderr}");
        assert_eq!(stdout, "---\nAda\n---\nLinus\n");

        let (status, stdout, stderr) = invoke(&["yaml-rt", "get", "--query", "$.missing"], input);
        assert_eq!(status, 0, "{stderr}");
        assert!(stdout.is_empty());

        let (status, stdout, stderr) = invoke(&["yaml-rt", "get", "--query", "$"], "---\n");
        assert_eq!(status, 0, "{stderr}");
        assert_eq!(stdout, "---\n");
    }

    #[test]
    fn query_targeted_value_mutations_are_atomic() {
        let input = "items: [{enabled: false}, {enabled: false}]\n";
        for operation in ["add", "replace"] {
            let (status, stdout, stderr) = invoke(
                &[
                    "yaml-rt",
                    operation,
                    "--query",
                    "$.items[*].enabled",
                    "--value",
                    "true",
                ],
                input,
            );
            assert_eq!(status, 0, "{stderr}");
            assert_eq!(stdout, "items: [{enabled: true}, {enabled: true}]\n");
        }

        let (status, stdout, stderr) = invoke(
            &[
                "yaml-rt",
                "replace",
                "--query",
                "$.missing",
                "--value",
                "true",
            ],
            input,
        );
        assert_eq!(status, FAILURE);
        assert!(stdout.is_empty());
        assert!(stderr.contains("query matched no nodes"));
    }

    #[test]
    fn query_targeted_remove_normalizes_and_orders_matches() {
        let (status, stdout, stderr) = invoke(
            &["yaml-rt", "remove", "--query", "$.items[0,2,0]"],
            "items: [a, b, c, d]\n",
        );
        assert_eq!(status, 0, "{stderr}");
        assert_eq!(stdout, "items: [b, d]\n");

        let (status, stdout, stderr) = invoke(
            &["yaml-rt", "remove", "--query", "$..*"],
            "root: {child: x}\nuntouched: y\n",
        );
        assert_eq!(status, 0, "{stderr}");
        assert!(stdout.is_empty());
    }

    #[test]
    fn query_targeted_test_requires_matches_and_tests_every_node() {
        let input = "values: [1, 1, 2]\n";
        let (status, stdout, stderr) = invoke(
            &[
                "yaml-rt",
                "test",
                "--query",
                "$.values[0,1]",
                "--value",
                "1",
            ],
            input,
        );
        assert_eq!(status, 0, "{stderr}");
        assert!(stdout.is_empty());

        let (status, stdout, stderr) = invoke(
            &["yaml-rt", "test", "--query", "$.values[*]", "--value", "1"],
            input,
        );
        assert_eq!(status, FAILURE);
        assert!(stdout.is_empty());
        assert!(stderr.contains("/values/2"));

        let (status, stdout, stderr) = invoke(
            &["yaml-rt", "test", "--query", "$.missing", "--value", "1"],
            input,
        );
        assert_eq!(status, FAILURE);
        assert!(stdout.is_empty());
        assert!(stderr.contains("query matched no nodes"));
    }

    #[test]
    fn query_targeted_commands_reject_extra_positionals_as_usage_errors() {
        let (status, stdout, stderr) = invoke(
            &["yaml-rt", "get", "--query", "$.value", "first"],
            "value: 1\n",
        );
        assert_eq!(status, USAGE);
        assert!(stdout.is_empty());
        assert!(stderr.contains("at most one positional FILE"));
    }

    #[test]
    fn query_targeted_commands_report_query_errors_before_output() {
        let (status, stdout, stderr) =
            invoke(&["yaml-rt", "get", "--query", "not-jsonpath"], "value: 1\n");
        assert_eq!(status, FAILURE);
        assert!(stdout.is_empty());
        assert!(stderr.contains("JSONPath"));

        let (status, stdout, stderr) = invoke(
            &["yaml-rt", "remove", "--query", "$.*"],
            "? [complex, key]\n: value\n",
        );
        assert_eq!(status, FAILURE);
        assert!(stdout.is_empty());
        assert!(stderr.contains("non-string key"));
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
    fn inline_patch_is_transactional() {
        let patch =
            "- {op: replace, path: /port, value: 9090}\n- {op: add, path: /debug, value: true}\n";
        let (status, stdout, stderr) = invoke(
            &["yaml-rt", "patch", "--patch", patch],
            "port: 8080 # keep\n",
        );
        assert_eq!(status, 0, "{stderr}");
        assert_eq!(stdout, "port: 9090 # keep\ndebug: true\n");

        let failing =
            "- {op: replace, path: /port, value: 9090}\n- {op: test, path: /port, value: 8080}\n";
        let (status, stdout, stderr) =
            invoke(&["yaml-rt", "patch", "--patch", failing], "port: 8080\n");
        assert_eq!(status, FAILURE);
        assert!(stdout.is_empty());
        assert!(stderr.contains("patch operation[1]"));
    }

    #[test]
    fn patch_source_is_required_and_exclusive() {
        let (status, _, stderr) = invoke(&["yaml-rt", "patch"], "{}\n");
        assert_eq!(status, USAGE);
        assert!(stderr.contains("required"));

        let (status, _, stderr) = invoke(
            &[
                "yaml-rt",
                "patch",
                "--patch",
                "[]",
                "--patch-file",
                "changes.yaml",
            ],
            "{}\n",
        );
        assert_eq!(status, USAGE);
        assert!(stderr.contains("cannot be used with"));
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
