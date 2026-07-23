use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use clap::{Arg, ArgAction, ArgGroup, ArgMatches, Command, error::ErrorKind};
use yaml_rt_core::{JsonPointer, YamlDoc, YamlFragment};

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
    let matches = match command().try_get_matches_from(args) {
        Ok(matches) => matches,
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
    match execute(&matches, stdin, stdout) {
        Ok(()) => 0,
        Err(RunError::BrokenPipe) => 0,
        Err(RunError::Message(message)) => {
            let _ = writeln!(stderr, "yaml-rt: {message}");
            FAILURE
        }
    }
}

fn command() -> Command {
    Command::new("yaml-rt")
        .about("Edit YAML through JSON Pointers while preserving presentation")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(read_command("get", "Print a selected YAML node", true))
        .subcommand(value_command("add", "Add or replace a value", true))
        .subcommand(mutation_command(
            "remove",
            "Remove an existing value",
            OperationArgs::Path,
        ))
        .subcommand(value_command("replace", "Replace an existing value", true))
        .subcommand(mutation_command(
            "move",
            "Move an existing value",
            OperationArgs::FromPath,
        ))
        .subcommand(mutation_command(
            "copy",
            "Copy an existing value",
            OperationArgs::FromPath,
        ))
        .subcommand(value_command(
            "test",
            "Test semantic equality at a path",
            false,
        ))
}

#[derive(Clone, Copy)]
enum OperationArgs {
    Path,
    FromPath,
}

fn base_command(name: &'static str, about: &'static str, args: OperationArgs) -> Command {
    let mut command = Command::new(name).about(about);
    match args {
        OperationArgs::Path => {
            command = command.arg(path_arg("path", 1));
        }
        OperationArgs::FromPath => {
            command = command.arg(path_arg("from", 1)).arg(path_arg("path", 2));
        }
    }
    command
        .arg(
            Arg::new("file")
                .value_name("FILE")
                .index(match args {
                    OperationArgs::Path => 2,
                    OperationArgs::FromPath => 3,
                })
                .help("Input YAML file; defaults to stdin"),
        )
        .arg(
            Arg::new("doc")
                .long("doc")
                .value_name("INDEX")
                .value_parser(clap::value_parser!(usize))
                .help("Zero-based YAML document index"),
        )
}

fn path_arg(name: &'static str, index: usize) -> Arg {
    Arg::new(name)
        .value_name(if name == "from" { "FROM" } else { "PATH" })
        .index(index)
        .required(true)
        .allow_hyphen_values(true)
}

fn output_arg() -> Arg {
    Arg::new("output")
        .short('o')
        .long("output")
        .value_name("FILE")
        .help("Write output to a file")
}

fn read_command(name: &'static str, about: &'static str, output: bool) -> Command {
    let command = base_command(name, about, OperationArgs::Path);
    if output {
        command.arg(output_arg())
    } else {
        command
    }
}

fn mutation_command(name: &'static str, about: &'static str, args: OperationArgs) -> Command {
    base_command(name, about, args).arg(output_arg()).arg(
        Arg::new("in-place")
            .short('i')
            .long("in-place")
            .action(ArgAction::SetTrue)
            .conflicts_with("output")
            .help("Atomically replace the input file"),
    )
}

fn value_command(name: &'static str, about: &'static str, mutation: bool) -> Command {
    let command = base_command(name, about, OperationArgs::Path)
        .arg(
            Arg::new("value")
                .long("value")
                .value_name("YAML")
                .allow_hyphen_values(true)
                .help("Complete YAML node"),
        )
        .arg(
            Arg::new("value-file")
                .long("value-file")
                .value_name("FILE")
                .help("Read the YAML node from a file"),
        )
        .group(
            ArgGroup::new("value-source")
                .args(["value", "value-file"])
                .required(true)
                .multiple(false),
        );
    if mutation {
        command.arg(output_arg()).arg(
            Arg::new("in-place")
                .short('i')
                .long("in-place")
                .action(ArgAction::SetTrue)
                .conflicts_with("output")
                .help("Atomically replace the input file"),
        )
    } else {
        command
    }
}

fn execute(
    matches: &ArgMatches,
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
) -> Result<(), RunError> {
    let (operation, arguments) = matches
        .subcommand()
        .ok_or_else(|| RunError::message("missing subcommand"))?;
    let input_path = arguments.get_one::<String>("file").map(PathBuf::from);
    let target_uses_stdin = input_path
        .as_deref()
        .is_none_or(|path| path == Path::new("-"));
    let input = read_target(input_path.as_deref(), stdin)?;
    let mut doc = YamlDoc::parse_owned(input).map_err(RunError::display)?;
    let document = select_document(&doc, arguments.get_one::<usize>("doc").copied())?;

    let path = arguments
        .get_one::<String>("path")
        .map(|value| JsonPointer::parse(value))
        .transpose()
        .map_err(RunError::display)?;
    let from = arguments
        .try_get_one::<String>("from")
        .ok()
        .flatten()
        .map(|value| JsonPointer::parse(value))
        .transpose()
        .map_err(RunError::display)?;
    let value = read_value(arguments, target_uses_stdin, stdin)?;

    match operation {
        "get" => {
            let pointer = path.as_ref().expect("Clap requires path");
            let node = doc
                .resolve_pointer(document, pointer)
                .map_err(RunError::display)?;
            let output = doc.extract_node(node).map_err(RunError::display)?;
            write_result(
                output.as_bytes(),
                arguments.get_one::<String>("output").map(Path::new),
                input_path.as_deref(),
                stdout,
            )
        }
        "test" => {
            let equal = doc
                .test_at(
                    document,
                    path.as_ref().expect("Clap requires path"),
                    value.as_ref().expect("Clap requires a value"),
                )
                .map_err(RunError::display)?;
            if equal {
                Ok(())
            } else {
                Err(RunError::message(format!(
                    "test failed at {:?}: values are not semantically equal",
                    path.as_ref().map_or("", JsonPointer::as_str)
                )))
            }
        }
        "add" => {
            doc.add_at(
                document,
                path.as_ref().expect("Clap requires path"),
                value.as_ref().expect("Clap requires a value"),
            )
            .map_err(RunError::display)?;
            write_mutation(&doc, arguments, input_path.as_deref(), stdout)
        }
        "remove" => {
            doc.remove_at(document, path.as_ref().expect("Clap requires path"))
                .map_err(RunError::display)?;
            write_mutation(&doc, arguments, input_path.as_deref(), stdout)
        }
        "replace" => {
            doc.replace_at(
                document,
                path.as_ref().expect("Clap requires path"),
                value.as_ref().expect("Clap requires a value"),
            )
            .map_err(RunError::display)?;
            write_mutation(&doc, arguments, input_path.as_deref(), stdout)
        }
        "move" => {
            doc.move_at(
                document,
                from.as_ref().expect("Clap requires from"),
                path.as_ref().expect("Clap requires path"),
            )
            .map_err(RunError::display)?;
            write_mutation(&doc, arguments, input_path.as_deref(), stdout)
        }
        "copy" => {
            doc.copy_at(
                document,
                from.as_ref().expect("Clap requires from"),
                path.as_ref().expect("Clap requires path"),
            )
            .map_err(RunError::display)?;
            write_mutation(&doc, arguments, input_path.as_deref(), stdout)
        }
        _ => Err(RunError::message("unknown subcommand")),
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
    arguments: &ArgMatches,
    target_uses_stdin: bool,
    stdin: &mut dyn Read,
) -> Result<Option<YamlFragment>, RunError> {
    let input = if let Some(value) = arguments.try_get_one::<String>("value").ok().flatten() {
        Some(value.clone())
    } else if let Some(path) = arguments.try_get_one::<String>("value-file").ok().flatten() {
        if path == "-" {
            if target_uses_stdin {
                return Err(RunError::message(
                    "target YAML and --value-file cannot both read stdin",
                ));
            }
            Some(read_stream(stdin, "value stdin")?)
        } else {
            Some(fs::read_to_string(path).map_err(|error| {
                RunError::message(format!("cannot read value file {path}: {error}"))
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
    arguments: &ArgMatches,
    input: Option<&Path>,
    stdout: &mut dyn Write,
) -> Result<(), RunError> {
    if arguments.get_flag("in-place") {
        let input = input
            .filter(|path| *path != Path::new("-"))
            .ok_or_else(|| RunError::message("--in-place requires a real input filename"))?;
        atomic_replace(input, doc.as_source().as_bytes())
    } else {
        write_result(
            doc.as_source().as_bytes(),
            arguments.get_one::<String>("output").map(Path::new),
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
}
