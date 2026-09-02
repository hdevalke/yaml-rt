//! Browser command engine used by the yaml-rt playground.

use std::cmp::Ordering;
use std::collections::HashSet;

use wasm_bindgen::prelude::*;
use yaml_rt_core::{Diagnostic, DiagnosticKind, JsonPointer, YamlDoc, YamlFragment, YamlPatch};
use yaml_rt_rfc9535::{JsonPath, QueryMatches};

/// Structured command request shared by native tests and the WASM adapter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandRequest {
    pub source: String,
    pub document_index: usize,
    pub command: String,
    pub selector_kind: Option<String>,
    pub selector: Option<String>,
    pub from: Option<String>,
    pub destination: Option<String>,
    pub value: Option<String>,
    pub new_key: Option<String>,
    pub patch: Option<String>,
}

/// Structured command response returned by the command engine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandResult {
    pub ok: bool,
    pub output_yaml: String,
    pub command_output: String,
    pub matched_pointers: Vec<String>,
    pub document_count: usize,
    pub error_source: Option<String>,
    pub message: Option<String>,
    pub rendered_diagnostic: Option<String>,
    pub operation_index: Option<usize>,
    pub span_start: Option<u32>,
    pub span_end: Option<u32>,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

impl CommandResult {
    fn success(doc: &YamlDoc, command_output: String, matched_pointers: Vec<String>) -> Self {
        Self {
            ok: true,
            output_yaml: doc.to_string(),
            command_output,
            matched_pointers,
            document_count: doc.document_count(),
            ..Self::default()
        }
    }

    fn request_error(source: &str, message: impl Into<String>, document_count: usize) -> Self {
        Self {
            error_source: Some(source.to_owned()),
            message: Some(message.into()),
            document_count,
            ..Self::default()
        }
    }

    fn document_error(
        doc: &YamlDoc,
        message: impl Into<String>,
        source_span: Option<yaml_rt_core::Span>,
        fallback_source: &str,
        document_count: usize,
    ) -> Self {
        let message = message.into();
        let Some(span) = source_span else {
            return Self::request_error(fallback_source, message, document_count);
        };
        let position = doc.source().line_col(span.start as usize);
        let rendered_diagnostic = Diagnostic::new(DiagnosticKind::Semantic, &message, span)
            .with_position(position)
            .render(doc.as_source())
            .with_source_name("<input>")
            .to_string();
        Self {
            error_source: Some("document".to_owned()),
            message: Some(message),
            rendered_diagnostic: Some(rendered_diagnostic),
            span_start: Some(span.start),
            span_end: Some(span.end),
            line: Some(position.line),
            column: Some(position.column),
            document_count,
            ..Self::default()
        }
    }
}

/// Executes one playground command without filesystem access.
#[must_use]
pub fn execute(request: &CommandRequest) -> CommandResult {
    let mut doc = match YamlDoc::parse(&request.source) {
        Ok(doc) => doc,
        Err(error) => {
            let diagnostic = error.diagnostic;
            let rendered_diagnostic = diagnostic
                .render(&request.source)
                .with_source_name("<input>")
                .to_string();
            return CommandResult {
                error_source: Some("document".to_owned()),
                message: Some(diagnostic.to_string()),
                rendered_diagnostic: Some(rendered_diagnostic),
                span_start: Some(diagnostic.span.start),
                span_end: Some(diagnostic.span.end),
                line: diagnostic.position.map(|position| position.line),
                column: diagnostic.position.map(|position| position.column),
                ..CommandResult::default()
            };
        }
    };
    let document_count = doc.document_count();
    if request.command == "validate" {
        return CommandResult::success(&doc, "Valid YAML.".to_owned(), Vec::new());
    }
    if request.document_index >= document_count {
        return CommandResult::request_error(
            "document",
            format!(
                "document index {} is out of range for a stream with {document_count} document(s)",
                request.document_index
            ),
            document_count,
        );
    }

    let command = request.command.as_str();
    if !matches!(
        command,
        "query"
            | "get"
            | "add"
            | "remove"
            | "replace"
            | "rename-key"
            | "move"
            | "copy"
            | "test"
            | "patch"
    ) {
        return CommandResult::request_error(
            "command",
            format!("unknown command {command:?}"),
            document_count,
        );
    }

    if command == "patch" {
        return execute_patch(&mut doc, request, document_count);
    }
    if matches!(command, "move" | "copy") {
        return execute_from_command(&mut doc, request, document_count);
    }

    let selector_kind = if command == "query" {
        "jsonpath"
    } else {
        request.selector_kind.as_deref().unwrap_or("pointer")
    };
    if !matches!(selector_kind, "pointer" | "jsonpath") {
        return CommandResult::request_error(
            "selector",
            "selector kind must be `pointer` or `jsonpath`",
            document_count,
        );
    }
    let Some(selector) = request.selector.as_deref() else {
        return CommandResult::request_error("selector", "a selector is required", document_count);
    };

    if selector_kind == "jsonpath" {
        execute_query_command(&mut doc, request, selector, document_count)
    } else {
        execute_pointer_command(&mut doc, request, selector, document_count)
    }
}

fn execute_patch(
    doc: &mut YamlDoc,
    request: &CommandRequest,
    document_count: usize,
) -> CommandResult {
    let Some(source) = request.patch.as_deref() else {
        return CommandResult::request_error(
            "patch",
            "a patch document is required",
            document_count,
        );
    };
    let patch = match YamlPatch::parse(source) {
        Ok(patch) => patch,
        Err(error) => {
            let span = error.span();
            let message = error.to_string();
            let rendered_diagnostic = span.map(|span| {
                Diagnostic::new(DiagnosticKind::Parser, &message, span)
                    .render(source)
                    .with_source_name("<patch>")
                    .to_string()
            });
            return CommandResult {
                error_source: Some("patch".to_owned()),
                message: Some(message),
                rendered_diagnostic,
                operation_index: error.operation_index(),
                span_start: span.map(|span| span.start),
                span_end: span.map(|span| span.end),
                document_count,
                ..CommandResult::default()
            };
        }
    };
    match doc.apply_patch(request.document_index, &patch) {
        Ok(()) => {
            CommandResult::success(doc, "Patch applied transactionally.".to_owned(), Vec::new())
        }
        Err(error) => {
            let span = error.span();
            let message = error.to_string();
            let rendered_diagnostic = span.map(|span| {
                Diagnostic::new(DiagnosticKind::Semantic, &message, span)
                    .render(source)
                    .with_source_name("<patch>")
                    .to_string()
            });
            CommandResult {
                error_source: Some("application".to_owned()),
                message: Some(message),
                rendered_diagnostic,
                operation_index: error.operation_index(),
                span_start: span.map(|span| span.start),
                span_end: span.map(|span| span.end),
                document_count,
                ..CommandResult::default()
            }
        }
    }
}

fn execute_from_command(
    doc: &mut YamlDoc,
    request: &CommandRequest,
    document_count: usize,
) -> CommandResult {
    if request.selector_kind.as_deref() == Some("jsonpath") {
        return CommandResult::request_error(
            "selector",
            format!("{} supports JSON Pointer only", request.command),
            document_count,
        );
    }
    let from = match parse_pointer(request.from.as_deref(), "source", document_count) {
        Ok(pointer) => pointer,
        Err(result) => return *result,
    };
    let destination = match parse_pointer(
        request.destination.as_deref(),
        "destination",
        document_count,
    ) {
        Ok(pointer) => pointer,
        Err(result) => return *result,
    };
    let result = if request.command == "move" {
        doc.move_at(request.document_index, &from, &destination)
    } else {
        doc.copy_at(request.document_index, &from, &destination)
    };
    match result {
        Ok(()) => CommandResult::success(doc, String::new(), Vec::new()),
        Err(error) => CommandResult::document_error(
            doc,
            error.to_string(),
            error.source_span(),
            "application",
            document_count,
        ),
    }
}

fn execute_pointer_command(
    doc: &mut YamlDoc,
    request: &CommandRequest,
    selector: &str,
    document_count: usize,
) -> CommandResult {
    if request.command == "query" {
        return CommandResult::request_error(
            "selector",
            "query requires an RFC 9535 JSONPath selector",
            document_count,
        );
    }
    let pointer = match JsonPointer::parse(selector) {
        Ok(pointer) => pointer,
        Err(error) => {
            return CommandResult::request_error("selector", error.to_string(), document_count);
        }
    };
    let pointers = vec![pointer.as_str().to_owned()];
    match request.command.as_str() {
        "get" => match doc.resolve_pointer(request.document_index, &pointer) {
            Ok(node) => match doc.extract_node(node) {
                Ok(output) => CommandResult::success(doc, output, pointers),
                Err(error) => {
                    CommandResult::request_error("application", error.to_string(), document_count)
                }
            },
            Err(error) => CommandResult::document_error(
                doc,
                error.to_string(),
                error.source_span(),
                "application",
                document_count,
            ),
        },
        "test" => {
            let value = match parse_value(request, document_count) {
                Ok(value) => value,
                Err(result) => return *result,
            };
            match doc.test_at(request.document_index, &pointer, &value) {
                Ok(true) => CommandResult::success(doc, "Test passed.".to_owned(), pointers),
                Ok(false) => CommandResult::request_error(
                    "application",
                    format!(
                        "test failed at {:?}: values are not semantically equal",
                        pointer.as_str()
                    ),
                    document_count,
                ),
                Err(error) => CommandResult::document_error(
                    doc,
                    error.to_string(),
                    error.source_span(),
                    "application",
                    document_count,
                ),
            }
        }
        "rename-key" => {
            let Some(new_key) = request.new_key.as_deref() else {
                return CommandResult::request_error(
                    "value",
                    "a replacement key is required",
                    document_count,
                );
            };
            match doc.rename_key_at(request.document_index, &pointer, new_key) {
                Ok(()) => CommandResult::success(doc, String::new(), pointers),
                Err(error) => CommandResult::document_error(
                    doc,
                    error.to_string(),
                    error.source_span(),
                    "application",
                    document_count,
                ),
            }
        }
        "add" | "replace" => {
            let value = match parse_value(request, document_count) {
                Ok(value) => value,
                Err(result) => return *result,
            };
            let result = if request.command == "add" {
                doc.add_at(request.document_index, &pointer, &value)
            } else {
                doc.replace_at(request.document_index, &pointer, &value)
            };
            match result {
                Ok(()) => CommandResult::success(doc, String::new(), pointers),
                Err(error) => CommandResult::document_error(
                    doc,
                    error.to_string(),
                    error.source_span(),
                    "application",
                    document_count,
                ),
            }
        }
        "remove" => match doc.remove_at(request.document_index, &pointer) {
            Ok(()) => CommandResult::success(doc, String::new(), pointers),
            Err(error) => CommandResult::document_error(
                doc,
                error.to_string(),
                error.source_span(),
                "application",
                document_count,
            ),
        },
        _ => CommandResult::request_error("command", "unsupported pointer command", document_count),
    }
}

fn execute_query_command(
    doc: &mut YamlDoc,
    request: &CommandRequest,
    selector: &str,
    document_count: usize,
) -> CommandResult {
    let query = match JsonPath::parse(selector) {
        Ok(query) => query,
        Err(error) => {
            return CommandResult::document_error(
                doc,
                error.to_string(),
                error.source_span(),
                "selector",
                document_count,
            );
        }
    };
    let matches = match query.query(doc, request.document_index) {
        Ok(matches) => matches,
        Err(error) => {
            return CommandResult::document_error(
                doc,
                error.to_string(),
                error.source_span(),
                "selector",
                document_count,
            );
        }
    };
    let pointers = matches
        .iter()
        .map(|matched| matched.pointer().as_str().to_owned())
        .collect::<Vec<_>>();

    match request.command.as_str() {
        "query" => match render_query_matches(doc, &matches) {
            Ok(output) => CommandResult::success(doc, output, pointers),
            Err(message) => CommandResult::request_error("application", message, document_count),
        },
        "get" => match render_get_matches(doc, &matches) {
            Ok(output) => CommandResult::success(doc, output, pointers),
            Err(message) => CommandResult::request_error("application", message, document_count),
        },
        "test" => {
            if matches.is_empty() {
                return no_matches(document_count);
            }
            let value = match parse_value(request, document_count) {
                Ok(value) => value,
                Err(result) => return *result,
            };
            for matched in &matches {
                let pointer = matched.pointer();
                match doc.test_at(request.document_index, pointer, &value) {
                    Ok(true) => {}
                    Ok(false) => {
                        return CommandResult::request_error(
                            "application",
                            format!(
                                "test failed at {:?}: values are not semantically equal",
                                pointer.as_str()
                            ),
                            document_count,
                        );
                    }
                    Err(error) => {
                        return CommandResult::document_error(
                            doc,
                            error.to_string(),
                            error.source_span(),
                            "application",
                            document_count,
                        );
                    }
                }
            }
            CommandResult::success(doc, "Test passed for every match.".to_owned(), pointers)
        }
        "rename-key" => {
            if matches.is_empty() {
                return no_matches(document_count);
            }
            let Some(new_key) = request.new_key.as_deref() else {
                return CommandResult::request_error(
                    "value",
                    "a replacement key is required",
                    document_count,
                );
            };
            let targets = matches
                .iter()
                .map(|matched| matched.pointer().clone())
                .collect::<Vec<_>>();
            match doc.rename_keys_at(request.document_index, &targets, new_key) {
                Ok(()) => CommandResult::success(doc, String::new(), pointers),
                Err(error) => CommandResult::document_error(
                    doc,
                    error.to_string(),
                    error.source_span(),
                    "application",
                    document_count,
                ),
            }
        }
        "add" | "replace" | "remove" => {
            if matches.is_empty() {
                return no_matches(document_count);
            }
            let value = if matches!(request.command.as_str(), "add" | "replace") {
                match parse_value(request, document_count) {
                    Ok(value) => Some(value),
                    Err(result) => return *result,
                }
            } else {
                None
            };
            let mut targets = normalized_targets(&matches);
            if request.command == "remove" {
                targets.sort_by(removal_order);
            }
            let mut work = doc.clone();
            for pointer in &targets {
                let result = match request.command.as_str() {
                    "add" => work.add_at(request.document_index, pointer, value.as_ref().unwrap()),
                    "replace" => {
                        work.replace_at(request.document_index, pointer, value.as_ref().unwrap())
                    }
                    "remove" => work.remove_at(request.document_index, pointer),
                    _ => unreachable!(),
                };
                if let Err(error) = result {
                    return CommandResult::document_error(
                        doc,
                        error.to_string(),
                        error.source_span(),
                        "application",
                        document_count,
                    );
                }
            }
            *doc = work;
            CommandResult::success(doc, String::new(), pointers)
        }
        _ => CommandResult::request_error(
            "command",
            format!("{} does not support JSONPath", request.command),
            document_count,
        ),
    }
}

fn parse_pointer(
    source: Option<&str>,
    name: &str,
    document_count: usize,
) -> Result<JsonPointer, Box<CommandResult>> {
    let Some(source) = source else {
        return Err(Box::new(CommandResult::request_error(
            "selector",
            format!("a {name} JSON Pointer is required"),
            document_count,
        )));
    };
    JsonPointer::parse(source).map_err(|error| {
        Box::new(CommandResult::request_error(
            "selector",
            error.to_string(),
            document_count,
        ))
    })
}

fn parse_value(
    request: &CommandRequest,
    document_count: usize,
) -> Result<YamlFragment, Box<CommandResult>> {
    let Some(value) = request.value.as_deref() else {
        return Err(Box::new(CommandResult::request_error(
            "value",
            "a YAML value is required",
            document_count,
        )));
    };
    YamlFragment::parse(value).map_err(|error| {
        Box::new(CommandResult::request_error(
            "value",
            error.to_string(),
            document_count,
        ))
    })
}

fn no_matches(document_count: usize) -> CommandResult {
    CommandResult::request_error("selector", "query matched no nodes", document_count)
}

fn render_query_matches(doc: &YamlDoc, matches: &QueryMatches) -> Result<String, String> {
    let mut output = String::new();
    for matched in matches {
        let value = matched
            .node()
            .map(|node| doc.extract_node(node))
            .transpose()
            .map_err(|error| error.to_string())?
            .unwrap_or_else(|| "null".to_owned());
        output.push_str(matched.pointer().as_str());
        output.push_str(": ");
        output.push_str(value.trim());
        output.push('\n');
    }
    Ok(output)
}

fn render_get_matches(doc: &YamlDoc, matches: &QueryMatches) -> Result<String, String> {
    let mut output = String::new();
    for matched in matches {
        output.push_str("---\n");
        if let Some(node) = matched.node() {
            let fragment = doc.extract_node(node).map_err(|error| error.to_string())?;
            output.push_str(&fragment);
            if !fragment.ends_with(['\n', '\r']) {
                output.push('\n');
            }
        }
    }
    Ok(output)
}

fn normalized_targets(matches: &QueryMatches) -> Vec<JsonPointer> {
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

fn removal_order(left: &JsonPointer, right: &JsonPointer) -> Ordering {
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
                if order != Ordering::Equal {
                    return order;
                }
            }
            Ordering::Equal
        })
}

/// JavaScript-facing response object.
#[wasm_bindgen]
pub struct WasmCommandResult(CommandResult);

#[wasm_bindgen]
impl WasmCommandResult {
    #[wasm_bindgen(getter)]
    pub fn ok(&self) -> bool {
        self.0.ok
    }
    #[wasm_bindgen(getter)]
    pub fn output_yaml(&self) -> String {
        self.0.output_yaml.clone()
    }
    #[wasm_bindgen(getter)]
    pub fn command_output(&self) -> String {
        self.0.command_output.clone()
    }
    #[wasm_bindgen(getter)]
    pub fn matched_pointers(&self) -> Vec<String> {
        self.0.matched_pointers.clone()
    }
    #[wasm_bindgen(getter)]
    pub fn document_count(&self) -> usize {
        self.0.document_count
    }
    #[wasm_bindgen(getter)]
    pub fn error_source(&self) -> Option<String> {
        self.0.error_source.clone()
    }
    #[wasm_bindgen(getter)]
    pub fn message(&self) -> Option<String> {
        self.0.message.clone()
    }
    #[wasm_bindgen(getter)]
    pub fn rendered_diagnostic(&self) -> Option<String> {
        self.0.rendered_diagnostic.clone()
    }
    #[wasm_bindgen(getter)]
    pub fn operation_index(&self) -> Option<usize> {
        self.0.operation_index
    }
    #[wasm_bindgen(getter)]
    pub fn span_start(&self) -> Option<u32> {
        self.0.span_start
    }
    #[wasm_bindgen(getter)]
    pub fn span_end(&self) -> Option<u32> {
        self.0.span_end
    }
    #[wasm_bindgen(getter)]
    pub fn line(&self) -> Option<usize> {
        self.0.line
    }
    #[wasm_bindgen(getter)]
    pub fn column(&self) -> Option<usize> {
        self.0.column
    }
}

/// Executes a command from JavaScript. Empty optional strings represent absent fields.
#[wasm_bindgen]
#[allow(clippy::too_many_arguments)]
pub fn run_command(
    source: String,
    document_index: usize,
    command: String,
    selector_kind: String,
    selector: String,
    from: String,
    destination: String,
    value: String,
    new_key: String,
    patch: String,
) -> WasmCommandResult {
    let optional = |value: String| (!value.is_empty()).then_some(value);
    WasmCommandResult(execute(&CommandRequest {
        source,
        document_index,
        command,
        selector_kind: optional(selector_kind),
        selector: optional(selector),
        from: optional(from),
        destination: optional(destination),
        value: optional(value),
        new_key: optional(new_key),
        patch: optional(patch),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(command: &str) -> CommandRequest {
        CommandRequest {
            source: "# services\nservices:\n  - {name: api, port: 80}\n  - {name: web, port: 81}\n"
                .to_owned(),
            command: command.to_owned(),
            ..CommandRequest::default()
        }
    }

    #[test]
    fn pointer_commands_preserve_surrounding_source() {
        let mut replace = request("replace");
        replace.selector_kind = Some("pointer".to_owned());
        replace.selector = Some("/services/0/port".to_owned());
        replace.value = Some("8080".to_owned());
        let result = execute(&replace);
        assert!(result.ok, "{:?}", result.message);
        assert!(result.output_yaml.starts_with("# services\n"));
        assert!(result.output_yaml.contains("port: 8080"));

        let mut get = request("get");
        get.selector = Some("/services/0/name".to_owned());
        let result = execute(&get);
        assert!(result.ok);
        assert_eq!(result.output_yaml, get.source);
        assert_eq!(result.command_output, "api");
    }

    #[test]
    fn validate_accepts_complete_yaml_streams_and_ignores_document_selection() {
        for source in ["", "name: api\n", "---\nname: api\n---\nname: web\n"] {
            let result = execute(&CommandRequest {
                source: source.to_owned(),
                document_index: usize::MAX,
                command: "validate".to_owned(),
                ..CommandRequest::default()
            });

            assert!(result.ok, "{source:?}: {:?}", result.message);
            assert_eq!(result.command_output, "Valid YAML.");
            assert_eq!(result.output_yaml, source);
            assert!(result.matched_pointers.is_empty());
        }
    }

    #[test]
    fn validate_reports_malformed_yaml_with_source_location() {
        let result = execute(&CommandRequest {
            source: "ports: [8080, , 8443]\n".to_owned(),
            command: "validate".to_owned(),
            ..CommandRequest::default()
        });

        assert!(!result.ok);
        assert_eq!(result.error_source.as_deref(), Some("document"));
        assert!(result.rendered_diagnostic.is_some());
        assert!(result.span_start.is_some());
        assert!(result.line.is_some());
        assert!(result.column.is_some());
    }

    #[test]
    fn pointer_get_aligns_keys_in_extracted_sequence_mapping() {
        let mut get = request("get");
        get.source =
            "services:\n  - name: api\n    port: 8080 # public endpoint\n    enabled: TRUE\n"
                .to_owned();
        get.selector_kind = Some("pointer".to_owned());
        get.selector = Some("/services/0".to_owned());

        let result = execute(&get);

        assert!(result.ok, "{:?}", result.message);
        assert_eq!(
            result.command_output,
            "name: api\nport: 8080 # public endpoint\nenabled: TRUE"
        );
        assert_eq!(result.output_yaml, get.source);
    }

    #[test]
    fn jsonpath_commands_mutate_all_matches_transactionally() {
        let mut replace = request("replace");
        replace.selector_kind = Some("jsonpath".to_owned());
        replace.selector = Some("$.services[*].port".to_owned());
        replace.value = Some("9090".to_owned());
        let result = execute(&replace);
        assert!(result.ok, "{:?}", result.message);
        assert_eq!(result.matched_pointers.len(), 2);
        assert_eq!(result.output_yaml.matches("port: 9090").count(), 2);
    }

    #[test]
    fn jsonpath_remove_deletes_complete_multiline_sequence_items() {
        let mut remove = request("remove");
        remove.selector_kind = Some("jsonpath".to_owned());
        remove.selector = Some("$.services[0,1]".to_owned());

        let result = execute(&remove);

        assert!(result.ok, "{:?}", result.message);
        assert_eq!(result.matched_pointers, ["/services/0", "/services/1"]);
        assert_eq!(result.output_yaml, "# services\nservices: []\n");
        YamlDoc::parse(&result.output_yaml).unwrap();
    }

    #[test]
    fn query_get_and_test_do_not_change_yaml() {
        for command in ["query", "get", "test"] {
            let mut value = request(command);
            value.selector_kind = Some("jsonpath".to_owned());
            value.selector = Some("$.services[*].port".to_owned());
            if command == "test" {
                value.selector = Some("$.services[0].port".to_owned());
                value.value = Some("80".to_owned());
            }
            let result = execute(&value);
            assert!(result.ok, "{command}: {:?}", result.message);
            assert_eq!(result.output_yaml, value.source);
        }
    }

    #[test]
    fn add_remove_rename_move_copy_and_patch_work() {
        let cases = [
            ("add", "/services/0/enabled", Some("true"), None),
            ("remove", "/services/0/port", None, None),
            ("rename-key", "/services/0/port", None, Some("listen")),
        ];
        for (command, selector, value, new_key) in cases {
            let mut request = request(command);
            request.selector = Some(selector.to_owned());
            request.value = value.map(str::to_owned);
            request.new_key = new_key.map(str::to_owned);
            assert!(execute(&request).ok, "{command}");
        }

        for command in ["move", "copy"] {
            let mut request = request(command);
            request.from = Some("/services/0/port".to_owned());
            request.destination = Some("/services/1/copied".to_owned());
            assert!(execute(&request).ok, "{command}");
        }

        let mut patch = request("patch");
        patch.patch = Some("- op: replace\n  path: /services/0/port\n  value: 443\n".to_owned());
        assert!(execute(&patch).ok);
    }

    #[test]
    fn selector_constraints_and_diagnostics_are_structured() {
        let mut move_request = request("move");
        move_request.selector_kind = Some("jsonpath".to_owned());
        move_request.from = Some("/services/0".to_owned());
        move_request.destination = Some("/services/1".to_owned());
        let result = execute(&move_request);
        assert!(!result.ok);
        assert_eq!(result.error_source.as_deref(), Some("selector"));

        let mut patch = request("patch");
        patch.patch = Some("- op: replace\n  path: $.services[*]\n  value: 1\n".to_owned());
        let result = execute(&patch);
        assert!(!result.ok);
        assert_eq!(result.error_source.as_deref(), Some("patch"));
        assert_eq!(result.operation_index, Some(0));

        let mut invalid = request("replace");
        invalid.source = "a: [\n".to_owned();
        let result = execute(&invalid);
        assert!(!result.ok);
        assert_eq!(result.error_source.as_deref(), Some("document"));
        assert!(result.line.is_some());
        let rendered = result.rendered_diagnostic.as_deref().unwrap();
        assert!(rendered.contains("error[parser]"), "{rendered}");
        assert!(rendered.contains("--> <input>:2:"), "{rendered}");
        assert!(rendered.contains("1 | a: ["), "{rendered}");
        assert!(rendered.contains('^'), "{rendered}");
        assert!(rendered.contains("expected"), "{rendered}");
    }

    fn assert_alias_document_diagnostic(result: &CommandResult) {
        assert!(!result.ok);
        assert_eq!(result.error_source.as_deref(), Some("document"));
        assert_eq!(result.span_start, Some(5));
        assert_eq!(result.span_end, Some(13));
        assert_eq!(result.line, Some(1));
        assert_eq!(result.column, Some(6));
        let rendered = result.rendered_diagnostic.as_deref().unwrap();
        assert!(rendered.contains("error[semantic]"), "{rendered}");
        assert!(rendered.contains("--> <input>:1:6"), "{rendered}");
        assert!(rendered.contains("1 | bad: *missing"), "{rendered}");
        assert!(rendered.contains("^^^^^^^^"), "{rendered}");
    }

    #[test]
    fn jsonpath_and_pointer_alias_errors_are_source_aware() {
        let mut query = request("query");
        query.source = "bad: *missing\n".to_owned();
        query.selector_kind = Some("jsonpath".to_owned());
        query.selector = Some("$".to_owned());
        assert_alias_document_diagnostic(&execute(&query));

        let mut get = request("get");
        get.source = "bad: *missing\n".to_owned();
        get.selector = Some("/bad/value".to_owned());
        assert_alias_document_diagnostic(&execute(&get));

        let mut remove = request("remove");
        remove.source = "bad: *missing\n".to_owned();
        remove.selector = Some("/bad/value".to_owned());
        assert_alias_document_diagnostic(&execute(&remove));
    }

    #[test]
    fn patch_diagnostics_render_spans_against_patch_source() {
        let mut malformed = request("patch");
        malformed.patch = Some("- op: replace\n  path: [\n".to_owned());
        let result = execute(&malformed);
        let rendered = result.rendered_diagnostic.as_deref().unwrap();
        assert_eq!(result.error_source.as_deref(), Some("patch"));
        assert!(rendered.contains("--> <patch>:3:"), "{rendered}");
        assert!(rendered.contains("2 |   path: ["), "{rendered}");

        let mut failed = request("patch");
        failed.patch = Some("- op: test\n  path: /services/0/port\n  value: 999\n".to_owned());
        let result = execute(&failed);
        let rendered = result.rendered_diagnostic.as_deref().unwrap();
        assert_eq!(result.error_source.as_deref(), Some("application"));
        assert!(rendered.contains("error[semantic]"), "{rendered}");
        assert!(rendered.contains("--> <patch>:1:"), "{rendered}");

        let mut unspanned = request("patch");
        unspanned.patch = None;
        let result = execute(&unspanned);
        assert!(result.rendered_diagnostic.is_none());
        assert!(result.message.is_some());
    }

    #[test]
    fn multi_document_selection_and_empty_query_errors_work() {
        let mut value = request("replace");
        value.source = "---\nname: one\n---\nname: two\n".to_owned();
        value.document_index = 1;
        value.selector = Some("/name".to_owned());
        value.value = Some("second".to_owned());
        let result = execute(&value);
        assert!(result.ok);
        assert_eq!(result.document_count, 2);
        assert!(result.output_yaml.contains("name: one"));
        assert!(result.output_yaml.contains("name: second"));

        let mut missing = request("remove");
        missing.selector_kind = Some("jsonpath".to_owned());
        missing.selector = Some("$.missing".to_owned());
        let result = execute(&missing);
        assert!(!result.ok);
        assert_eq!(result.message.as_deref(), Some("query matched no nodes"));
    }
}
