//! Command-line querying and editing operations for the `yaml-rt` binary.
//!
//! The binary searches YAML documents with `JSONPath`, applies JSON Pointer
//! operations, and executes transactional YAML or JSON patch documents while
//! retaining unrelated presentation. [`run`] is public so integrations can
//! supply their own argument and I/O streams.

use std::cmp::Ordering as CmpOrdering;
use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use clap::{Args, CommandFactory, Parser, Subcommand, error::ErrorKind};
use yaml_rt_core::{JsonPointer, YamlDoc, YamlFragment, YamlPatch};
use yaml_rt_rfc9535::QueryMatches;

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
    /// Input YAML file; defaults to stdin.
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
    let target = operation.target();
    let input_path = operation.input_path();
    let target_uses_stdin = input_path.is_none_or(|path| path == Path::new("-"));
    if matches!(operation, Operation::Patch(arguments) if arguments.source.patch_file.as_deref() == Some(Path::new("-")))
        && target_uses_stdin
    {
        return Err(RunError::message(
            "target YAML and --patch-file cannot both read stdin",
        ));
    }
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

    if let Operation::Patch(arguments) = operation {
        let patch = read_patch(&arguments.source, stdin)?;
        doc.apply_patch(document, &patch)
            .map_err(RunError::display)?;
        return write_mutation(&doc, &arguments.output, input_path, stdout);
    }

    let value = read_value(operation.value(), target_uses_stdin, stdin)?;
    if let Some(source) = operation.selection_query() {
        let matches = query_matches(&doc, document, source).map_err(RunError::display)?;
        return execute_query_targeted(
            operation,
            &mut doc,
            document,
            &matches,
            value.as_ref(),
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
        Operation::Patch(_) => unreachable!("patch returned before pointer operations"),
    }
}

impl Operation {
    fn validate(&self) -> Result<(), String> {
        let path = match self {
            Self::Get(args) => Some(&args.path),
            Self::Add(args) | Self::Replace(args) => Some(&args.value.path),
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
            Self::Remove(args) => args.path.pointer(),
            Self::Move(args) | Self::Copy(args) => &args.path.path,
            Self::Test(args) => args.path.pointer(),
        }
    }

    fn selection_query(&self) -> Option<&str> {
        match self {
            Self::Get(args) => args.path.query.as_deref(),
            Self::Add(args) | Self::Replace(args) => args.value.path.query.as_deref(),
            Self::Remove(args) => args.path.query.as_deref(),
            Self::Test(args) => args.path.query.as_deref(),
            _ => None,
        }
    }

    fn input_path(&self) -> Option<&Path> {
        match self {
            Self::Get(args) => args.path.input_path(),
            Self::Add(args) | Self::Replace(args) => args.value.path.input_path(),
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

fn execute_query_targeted(
    operation: &Operation,
    doc: &mut YamlDoc,
    document: usize,
    matches: &QueryMatches,
    value: Option<&YamlFragment>,
    input_path: Option<&Path>,
    stdout: &mut dyn Write,
) -> Result<(), RunError> {
    match operation {
        Operation::Get(arguments) => {
            let output = render_yaml_stream(doc, matches)?;
            write_result(
                output.as_bytes(),
                arguments.output.output.as_deref(),
                input_path,
                stdout,
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
            write_mutation(doc, &arguments.output, input_path, stdout)
        }
        Operation::Remove(arguments) => {
            apply_query_mutation(doc, document, matches, QueryMutation::Remove)?;
            write_mutation(doc, &arguments.output, input_path, stdout)
        }
        Operation::Replace(arguments) => {
            apply_query_mutation(
                doc,
                document,
                matches,
                QueryMutation::Replace(value.expect("Clap requires a value")),
            )?;
            write_mutation(doc, &arguments.output, input_path, stdout)
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
}

impl RunError {
    fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
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
            &["yaml-rt", "get", "--query", "$.value", "first", "second"],
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
