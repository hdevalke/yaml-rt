use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use yaml_rt_core::{DiagnosticKind, NodeId, SemanticKind, YamlDoc, YamlError, YamlScalarStyle};

/// Environment variable overriding the in-repo YAML Test Suite data checkout.
const SUITE_DIR_ENV: &str = "YAML_TEST_SUITE_DIR";
/// Optional comma-separated list of case ids to run, such as `MJS9` or `VJP3:00`.
const CASES_ENV: &str = "YAML_TEST_SUITE_CASES";
/// Set to `1` to run every discovered case. This is intentionally opt-in while
/// the parser is still an MVP subset.
const RUN_ALL_ENV: &str = "YAML_TEST_SUITE_RUN_ALL";
/// Set to `1` to compare optional YAML Test Suite `in.json` fixtures against
/// the semantic graph. This is opt-in while schema-compatible value rendering
/// is being expanded.
const CHECK_JSON_ENV: &str = "YAML_TEST_SUITE_CHECK_JSON";
/// Valid YAML Test Suite cases accepted as known failures while the parser,
/// composer, and schema layers are incomplete.
const EXPECTED_FAILURES: &[&str] = &[];

#[derive(Debug, Clone, PartialEq, Eq)]
struct SuiteCase {
    id: String,
    dir: PathBuf,
    input: PathBuf,
    test_event: PathBuf,
    json: Option<PathBuf>,
    is_error: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum FailureCategory {
    SourceValidation,
    Lexer,
    Parser,
    SemanticGraph,
    SchemaOrScalarDecode,
    EmitterRoundTrip,
    EventMismatch,
    InvalidAccepted,
    HarnessIo,
}

impl FailureCategory {
    const ALL: [Self; 9] = [
        Self::SourceValidation,
        Self::Lexer,
        Self::Parser,
        Self::SemanticGraph,
        Self::SchemaOrScalarDecode,
        Self::EmitterRoundTrip,
        Self::EventMismatch,
        Self::InvalidAccepted,
        Self::HarnessIo,
    ];
}

impl std::fmt::Display for FailureCategory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::SourceValidation => "SourceValidation",
            Self::Lexer => "Lexer",
            Self::Parser => "Parser",
            Self::SemanticGraph => "SemanticGraph",
            Self::SchemaOrScalarDecode => "SchemaOrScalarDecode",
            Self::EmitterRoundTrip => "EmitterRoundTrip",
            Self::EventMismatch => "EventMismatch",
            Self::InvalidAccepted => "InvalidAccepted",
            Self::HarnessIo => "HarnessIo",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ClassifiedFailure {
    category: FailureCategory,
    message: String,
}

impl ClassifiedFailure {
    fn new(category: FailureCategory, message: impl Into<String>) -> Self {
        Self {
            category,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ClassifiedFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "[{}] {}", self.category, self.message)
    }
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
    let mut unexpected_passes = Vec::new();
    let mut ran = 0usize;
    for case in cases {
        if !run_all && !selected.iter().any(|selected| selected == &case.id) {
            continue;
        }
        ran += 1;
        let expected_failure = EXPECTED_FAILURES.contains(&case.id.as_str());
        match run_case(&case) {
            Ok(()) if expected_failure => {
                unexpected_passes.push(format!("{} ({})", case.id, case.dir.display()));
            }
            Ok(()) => {}
            Err(error) if expected_failure => {
                eprintln!(
                    "expected YAML Test Suite failure: {} ({}): {error}",
                    case.id,
                    case.dir.display()
                );
            }
            Err(error) => {
                failures.push((
                    error.category,
                    format!("{} ({}): {error}", case.id, case.dir.display()),
                ));
            }
        }
    }

    println!(
        "ran: {}, failed: {}, success: {}",
        ran,
        failures.len() + unexpected_passes.len(),
        ran - failures.len() - unexpected_passes.len()
    );

    assert!(
        ran > 0,
        "no YAML Test Suite cases matched {CASES_ENV}={}",
        selected.join(",")
    );

    if !failures.is_empty() {
        let failure_count = failures.len();
        let failure_summary = failure_category_summary(&failures);
        let failure_details = failures
            .into_iter()
            .map(|(_, failure)| failure)
            .collect::<Vec<_>>()
            .join("\n");
        panic!(
            "{} YAML Test Suite case(s) failed:\n{}\n\n{}",
            failure_count, failure_summary, failure_details
        );
    }

    if !unexpected_passes.is_empty() {
        panic!(
            "{} expected YAML Test Suite failure(s) now pass; remove them from EXPECTED_FAILURES:\n{}",
            unexpected_passes.len(),
            unexpected_passes.join("\n")
        );
    }
}

fn failure_category_summary(failures: &[(FailureCategory, String)]) -> String {
    let mut summary = String::from("failure categories:");
    for category in FailureCategory::ALL {
        let count = failures
            .iter()
            .filter(|(failure_category, _)| *failure_category == category)
            .count();
        if count > 0 {
            summary.push_str(&format!("\n  {category}: {count}"));
        }
    }
    summary
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
            test_event: dir.join("test.event"),
            json: dir.join("in.json").is_file().then(|| dir.join("in.json")),
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

fn run_case(case: &SuiteCase) -> Result<(), ClassifiedFailure> {
    let input = fs::read_to_string(&case.input).map_err(|error| {
        ClassifiedFailure::new(
            FailureCategory::HarnessIo,
            format!("failed to read {}: {error}", case.input.display()),
        )
    })?;
    let parsed = YamlDoc::parse(&input);

    if case.is_error {
        if parsed.is_ok() {
            return Err(ClassifiedFailure::new(
                FailureCategory::InvalidAccepted,
                "expected parse error, but parser accepted the case",
            ));
        }
        return Ok(());
    }

    let doc = parsed.map_err(|error| {
        ClassifiedFailure::new(
            failure_category_for_yaml_error(&error),
            format!("expected valid parse: {error}"),
        )
    })?;
    let output = doc.to_string();
    if output != input {
        return Err(ClassifiedFailure::new(
            FailureCategory::EmitterRoundTrip,
            "valid case did not round-trip byte-identically",
        ));
    }
    let expected_events = fs::read_to_string(&case.test_event).map_err(|error| {
        ClassifiedFailure::new(
            FailureCategory::HarnessIo,
            format!("failed to read {}: {error}", case.test_event.display()),
        )
    })?;
    let actual_events = doc.events_to_test_string();
    if actual_events != expected_events {
        return Err(ClassifiedFailure::new(
            FailureCategory::EventMismatch,
            format!(
                "valid case event stream differed\nexpected:\n{expected_events}\nactual:\n{actual_events}"
            ),
        ));
    }

    if env::var_os(CHECK_JSON_ENV).is_some_and(|value| value == "1") {
        if let Some(json) = &case.json {
            assert_json_fixture_matches(&doc, json)?;
        }
    }

    Ok(())
}

fn failure_category_for_yaml_error(error: &YamlError) -> FailureCategory {
    match error.diagnostic.kind {
        DiagnosticKind::Source => FailureCategory::SourceValidation,
        DiagnosticKind::Lexer => FailureCategory::Lexer,
        DiagnosticKind::Parser => FailureCategory::Parser,
        DiagnosticKind::Semantic => FailureCategory::SemanticGraph,
        DiagnosticKind::Typed => FailureCategory::SchemaOrScalarDecode,
        DiagnosticKind::Emitter => FailureCategory::EmitterRoundTrip,
    }
}

fn assert_json_fixture_matches(doc: &YamlDoc, fixture: &Path) -> Result<(), ClassifiedFailure> {
    let expected = fs::read_to_string(fixture).map_err(|error| {
        ClassifiedFailure::new(
            FailureCategory::HarnessIo,
            format!("failed to read {}: {error}", fixture.display()),
        )
    })?;
    let expected = JsonParser::new(&expected).parse().map_err(|error| {
        ClassifiedFailure::new(
            FailureCategory::HarnessIo,
            format!("failed to parse {}: {error}", fixture.display()),
        )
    })?;
    let actual = graph_to_json(doc)
        .map_err(|error| ClassifiedFailure::new(FailureCategory::SchemaOrScalarDecode, error))?;

    if actual != expected {
        return Err(ClassifiedFailure::new(
            FailureCategory::SchemaOrScalarDecode,
            format!(
                "JSON-compatible value differed\nexpected:\n{}\nactual:\n{}",
                expected.to_canonical_string(),
                actual.to_canonical_string()
            ),
        ));
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
enum JsonValue {
    Stream(Vec<JsonValue>),
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Array(Vec<JsonValue>),
    Object(Vec<(String, JsonValue)>),
}

impl JsonValue {
    fn to_canonical_string(&self) -> String {
        match self {
            Self::Stream(values) => values
                .iter()
                .map(Self::to_canonical_string)
                .collect::<Vec<_>>()
                .join("\n"),
            Self::Null => "null".to_owned(),
            Self::Bool(value) => value.to_string(),
            Self::Number(value) => value.clone(),
            Self::String(value) => format!("\"{}\"", escape_json_string(value)),
            Self::Array(items) => {
                let items = items
                    .iter()
                    .map(Self::to_canonical_string)
                    .collect::<Vec<_>>()
                    .join(",");
                format!("[{items}]")
            }
            Self::Object(entries) => {
                let entries = entries
                    .iter()
                    .map(|(key, value)| {
                        format!(
                            "\"{}\":{}",
                            escape_json_string(key),
                            value.to_canonical_string()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                format!("{{{entries}}}")
            }
        }
    }
}

fn graph_to_json(doc: &YamlDoc) -> Result<JsonValue, String> {
    let mut context = JsonRenderContext::default();
    let documents = doc.documents().collect::<Vec<_>>();
    match documents.as_slice() {
        [] => Ok(JsonValue::Stream(Vec::new())),
        [document] => graph_node_to_json(doc, *document, &mut context),
        documents => documents
            .iter()
            .map(|document| graph_node_to_json(doc, *document, &mut context))
            .collect::<Result<Vec<_>, _>>()
            .map(JsonValue::Stream),
    }
}

#[derive(Debug, Default)]
struct JsonRenderContext {
    anchors: BTreeMap<String, NodeId>,
}

fn graph_node_to_json(
    doc: &YamlDoc,
    node: NodeId,
    context: &mut JsonRenderContext,
) -> Result<JsonValue, String> {
    let semantic = doc
        .semantic_kind(node)
        .ok_or_else(|| format!("missing semantic node {}", node.as_usize()))?;
    match semantic {
        SemanticKind::Document => {
            let children = doc
                .children(node)
                .filter(|child| doc.semantic_kind(*child).is_some())
                .collect::<Vec<_>>();
            match children.as_slice() {
                [] => Ok(JsonValue::Null),
                [child] => graph_node_to_json(doc, *child, context),
                _ => {
                    Err("JSON fixture comparison only supports single-document content".to_owned())
                }
            }
        }
        SemanticKind::Mapping { .. } => {
            if let Some(anchor) = doc.anchor(node) {
                context.anchors.insert(anchor.to_owned(), node);
            }
            let mut object = Vec::new();
            for (key, value) in doc.mapping_entries(node) {
                object.push((
                    graph_key_to_json_key(doc, key, context)?,
                    graph_node_to_json(doc, value, context)?,
                ));
            }
            Ok(JsonValue::Object(sort_json_object_entries(object)))
        }
        SemanticKind::Sequence { .. } => {
            if let Some(anchor) = doc.anchor(node) {
                context.anchors.insert(anchor.to_owned(), node);
            }
            doc.sequence_items(node)
                .map(|item| graph_node_to_json(doc, item, context))
                .collect::<Result<Vec<_>, _>>()
                .map(JsonValue::Array)
        }
        SemanticKind::Scalar { style } => {
            if let Some(anchor) = doc.anchor(node) {
                context.anchors.insert(anchor.to_owned(), node);
            }
            let value = doc.scalar_value(node).map_err(|error| error.to_string())?;
            let tag = doc.resolved_tag(node).map_err(|error| error.to_string())?;
            scalar_to_json(style, &value, tag.as_deref())
        }
        SemanticKind::Alias => {
            let name = doc.alias_name(node).unwrap_or_default();
            let target = doc
                .resolve_alias(node)
                .ok_or_else(|| format!("alias `{name}` references an unknown anchor"))?;
            graph_node_to_json(doc, target, context)
        }
    }
}

fn graph_key_to_json_key(
    doc: &YamlDoc,
    node: NodeId,
    context: &mut JsonRenderContext,
) -> Result<String, String> {
    let semantic = doc
        .semantic_kind(node)
        .ok_or_else(|| format!("missing semantic key node {}", node.as_usize()))?;
    match semantic {
        SemanticKind::Scalar { .. } => {
            if let Some(anchor) = doc.anchor(node) {
                context.anchors.insert(anchor.to_owned(), node);
            }
            doc.scalar_value(node)
                .map(|value| value.into_owned())
                .map_err(|error| error.to_string())
        }
        SemanticKind::Alias => {
            let name = doc.alias_name(node).unwrap_or_default();
            let target = doc
                .resolve_alias(node)
                .ok_or_else(|| format!("alias key `{name}` references an unknown anchor"))?;
            graph_key_to_json_key(doc, target, context)
        }
        _ => Err("non-scalar mapping keys cannot be rendered as JSON object keys".to_owned()),
    }
}

fn scalar_to_json(
    style: YamlScalarStyle,
    value: &str,
    tag: Option<&str>,
) -> Result<JsonValue, String> {
    match tag {
        Some("tag:yaml.org,2002:null") => return Ok(JsonValue::Null),
        Some("tag:yaml.org,2002:bool") => return bool_to_json(value),
        Some("tag:yaml.org,2002:int") => return int_to_json(value),
        Some("tag:yaml.org,2002:float") => return float_to_json(value),
        Some("tag:yaml.org,2002:str" | "!") => return Ok(JsonValue::String(value.to_owned())),
        _ => {}
    }

    if !matches!(style, YamlScalarStyle::Plain) {
        return Ok(JsonValue::String(value.to_owned()));
    }

    if matches!(value, "" | "~" | "null" | "Null" | "NULL") {
        Ok(JsonValue::Null)
    } else if matches!(
        value,
        "true" | "True" | "TRUE" | "false" | "False" | "FALSE"
    ) {
        bool_to_json(value)
    } else if looks_like_int(value) {
        int_to_json(value)
    } else if looks_like_float(value) {
        float_to_json(value)
    } else {
        Ok(JsonValue::String(value.to_owned()))
    }
}

fn bool_to_json(value: &str) -> Result<JsonValue, String> {
    match value {
        "true" | "True" | "TRUE" => Ok(JsonValue::Bool(true)),
        "false" | "False" | "FALSE" => Ok(JsonValue::Bool(false)),
        _ => Err(format!("scalar `{value}` is not a JSON boolean")),
    }
}

fn int_to_json(value: &str) -> Result<JsonValue, String> {
    parse_yaml_int(value)
        .map(|number| JsonValue::Number(number.to_string()))
        .ok_or_else(|| format!("scalar `{value}` is not a JSON integer"))
}

fn float_to_json(value: &str) -> Result<JsonValue, String> {
    let number = value
        .parse::<f64>()
        .map_err(|_| format!("scalar `{value}` is not a JSON float"))?;
    if !number.is_finite() {
        return Err(format!("scalar `{value}` is not a finite JSON float"));
    }
    Ok(JsonValue::Number(normalize_json_number(value)))
}

fn looks_like_int(value: &str) -> bool {
    let digits = value.strip_prefix('-').unwrap_or(value);
    if let Some(hex) = digits
        .strip_prefix("0x")
        .or_else(|| digits.strip_prefix("0X"))
    {
        !hex.is_empty() && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
    } else {
        !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
    }
}

fn looks_like_float(value: &str) -> bool {
    value.bytes().any(|byte| matches!(byte, b'.' | b'e' | b'E')) && value.parse::<f64>().is_ok()
}

fn normalize_json_number(value: &str) -> String {
    let normalized = if value.contains(['e', 'E']) {
        value.to_ascii_lowercase()
    } else if let Some((integer, fraction)) = value.split_once('.') {
        let fraction = fraction.trim_end_matches('0');
        if fraction.is_empty() {
            integer.to_owned()
        } else {
            format!("{integer}.{fraction}")
        }
    } else {
        value.to_owned()
    };
    if normalized == "-0" {
        "0".to_owned()
    } else {
        normalized
    }
}

fn parse_yaml_int(value: &str) -> Option<i64> {
    let (sign, digits) = if let Some(digits) = value.strip_prefix('-') {
        (-1i64, digits)
    } else if let Some(digits) = value.strip_prefix('+') {
        (1i64, digits)
    } else {
        (1i64, value)
    };
    if let Some(hex) = digits
        .strip_prefix("0x")
        .or_else(|| digits.strip_prefix("0X"))
    {
        i64::from_str_radix(hex, 16)
            .ok()
            .and_then(|number| number.checked_mul(sign))
    } else {
        value.parse::<i64>().ok()
    }
}

fn escape_json_string(value: &str) -> String {
    let mut escaped = String::new();
    for char in value.chars() {
        match char {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            '\u{08}' => escaped.push_str("\\b"),
            '\u{0c}' => escaped.push_str("\\f"),
            char if char < ' ' => escaped.push_str(&format!("\\u{:04x}", char as u32)),
            char => escaped.push(char),
        }
    }
    escaped
}

struct JsonParser<'a> {
    input: &'a str,
    index: usize,
}

impl<'a> JsonParser<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, index: 0 }
    }

    fn parse(mut self) -> Result<JsonValue, String> {
        self.skip_ws();
        if self.index == self.input.len() {
            return Ok(JsonValue::Stream(Vec::new()));
        }
        let mut values = Vec::new();
        values.push(self.parse_value()?);
        loop {
            self.skip_ws();
            if self.index == self.input.len() {
                return if values.len() == 1 {
                    Ok(values.pop().expect("one parsed JSON value exists"))
                } else {
                    Ok(JsonValue::Stream(values))
                };
            }
            values.push(self.parse_value()?);
        }
    }

    fn parse_single(mut self) -> Result<JsonValue, String> {
        let value = self.parse_value()?;
        self.skip_ws();
        if self.index == self.input.len() {
            Ok(value)
        } else {
            Err(format!("trailing JSON content at byte {}", self.index))
        }
    }

    fn parse_value(&mut self) -> Result<JsonValue, String> {
        self.skip_ws();
        match self.peek() {
            Some(b'n') => self.parse_literal("null", JsonValue::Null),
            Some(b't') => self.parse_literal("true", JsonValue::Bool(true)),
            Some(b'f') => self.parse_literal("false", JsonValue::Bool(false)),
            Some(b'"') => self.parse_string().map(JsonValue::String),
            Some(b'[') => self.parse_array(),
            Some(b'{') => self.parse_object(),
            Some(b'-' | b'0'..=b'9') => self.parse_number().map(JsonValue::Number),
            Some(byte) => Err(format!(
                "unexpected JSON byte `{}` at byte {}",
                char::from(byte),
                self.index
            )),
            None => Err("unexpected end of JSON input".to_owned()),
        }
    }

    fn parse_literal(&mut self, literal: &str, value: JsonValue) -> Result<JsonValue, String> {
        if self.input[self.index..].starts_with(literal) {
            self.index += literal.len();
            Ok(value)
        } else {
            Err(format!("expected `{literal}` at byte {}", self.index))
        }
    }

    fn parse_array(&mut self) -> Result<JsonValue, String> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        loop {
            self.skip_ws();
            if self.consume(b']') {
                return Ok(JsonValue::Array(items));
            }
            items.push(self.parse_value()?);
            self.skip_ws();
            if self.consume(b']') {
                return Ok(JsonValue::Array(items));
            }
            self.expect(b',')?;
        }
    }

    fn parse_object(&mut self) -> Result<JsonValue, String> {
        self.expect(b'{')?;
        let mut entries = Vec::new();
        loop {
            self.skip_ws();
            if self.consume(b'}') {
                return Ok(JsonValue::Object(entries));
            }
            let key = self.parse_string()?;
            self.skip_ws();
            self.expect(b':')?;
            entries.push((key, self.parse_value()?));
            self.skip_ws();
            if self.consume(b'}') {
                return Ok(JsonValue::Object(sort_json_object_entries(entries)));
            }
            self.expect(b',')?;
        }
    }

    fn parse_string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut output = String::new();
        while let Some(byte) = self.next() {
            match byte {
                b'"' => return Ok(output),
                b'\\' => output.push(self.parse_escape()?),
                0x00..=0x1f => {
                    return Err(format!(
                        "unescaped control character in JSON string at byte {}",
                        self.index - 1
                    ));
                }
                byte if byte.is_ascii() => output.push(char::from(byte)),
                _ => {
                    let char = self.input[self.index - 1..]
                        .chars()
                        .next()
                        .ok_or_else(|| "invalid UTF-8 in JSON string".to_owned())?;
                    self.index = self.index - 1 + char.len_utf8();
                    output.push(char);
                }
            }
        }
        Err("unterminated JSON string".to_owned())
    }

    fn parse_escape(&mut self) -> Result<char, String> {
        match self.next() {
            Some(b'"') => Ok('"'),
            Some(b'\\') => Ok('\\'),
            Some(b'/') => Ok('/'),
            Some(b'b') => Ok('\u{08}'),
            Some(b'f') => Ok('\u{0c}'),
            Some(b'n') => Ok('\n'),
            Some(b'r') => Ok('\r'),
            Some(b't') => Ok('\t'),
            Some(b'u') => self.parse_unicode_escape(),
            Some(byte) => Err(format!("invalid JSON escape `\\{}`", char::from(byte))),
            None => Err("unterminated JSON escape".to_owned()),
        }
    }

    fn parse_unicode_escape(&mut self) -> Result<char, String> {
        let high = self.parse_hex_quad()?;
        let codepoint = if (0xd800..=0xdbff).contains(&high) {
            let checkpoint = self.index;
            if self.next() != Some(b'\\') || self.next() != Some(b'u') {
                return Err(format!(
                    "high surrogate at byte {checkpoint} is not followed by low surrogate"
                ));
            }
            let low = self.parse_hex_quad()?;
            if !(0xdc00..=0xdfff).contains(&low) {
                return Err(format!("invalid low surrogate U+{low:04X}"));
            }
            0x10000 + (((high - 0xd800) << 10) | (low - 0xdc00))
        } else {
            high
        };
        char::from_u32(codepoint).ok_or_else(|| format!("invalid Unicode scalar U+{codepoint:X}"))
    }

    fn parse_hex_quad(&mut self) -> Result<u32, String> {
        let mut value = 0u32;
        for _ in 0..4 {
            let byte = self
                .next()
                .ok_or_else(|| "truncated JSON Unicode escape".to_owned())?;
            value = (value << 4)
                | char::from(byte).to_digit(16).ok_or_else(|| {
                    format!("invalid JSON Unicode hex digit `{}`", char::from(byte))
                })?;
        }
        Ok(value)
    }

    fn parse_number(&mut self) -> Result<String, String> {
        let start = self.index;
        self.consume(b'-');
        match self.peek() {
            Some(b'0') => {
                self.index += 1;
            }
            Some(b'1'..=b'9') => {
                self.index += 1;
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.index += 1;
                }
            }
            _ => return Err(format!("invalid JSON number at byte {start}")),
        }
        if self.consume(b'.') {
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(format!("invalid JSON fraction at byte {}", self.index));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.index += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.index += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.index += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(format!("invalid JSON exponent at byte {}", self.index));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.index += 1;
            }
        }
        Ok(normalize_json_number(&self.input[start..self.index]))
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.index += 1;
        }
    }

    fn expect(&mut self, expected: u8) -> Result<(), String> {
        if self.consume(expected) {
            Ok(())
        } else {
            Err(format!(
                "expected `{}` at byte {}",
                char::from(expected),
                self.index
            ))
        }
    }

    fn consume(&mut self, expected: u8) -> bool {
        if self.peek() == Some(expected) {
            self.index += 1;
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.as_bytes().get(self.index).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.index += 1;
        Some(byte)
    }
}

fn sort_json_object_entries(mut entries: Vec<(String, JsonValue)>) -> Vec<(String, JsonValue)> {
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
}

#[cfg(test)]
mod json_fixture_tests {
    use super::*;

    fn render(input: &str) -> JsonValue {
        let doc = YamlDoc::parse(input).expect("valid YAML");
        graph_to_json(&doc).expect("YAML graph should render to JSON")
    }

    #[test]
    fn json_renderer_escapes_strings() {
        assert_eq!(
            render("text: \"a\\n\\t\\\\b\"\n").to_canonical_string(),
            "{\"text\":\"a\\n\\t\\\\b\"}"
        );
    }

    #[test]
    fn json_renderer_resolves_core_scalar_values() {
        assert_eq!(
            render("nullish:\nbool: true\nint: 0x2A\nfloat: 450.00\n").to_canonical_string(),
            "{\"bool\":true,\"float\":450,\"int\":42,\"nullish\":null}"
        );
    }

    #[test]
    fn json_renderer_handles_nested_collections() {
        assert_eq!(
            render("- name: Mark\n  scores: [1, 2]\n").to_canonical_string(),
            "[{\"name\":\"Mark\",\"scores\":[1,2]}]"
        );
    }

    #[test]
    fn json_renderer_treats_empty_document_as_null() {
        assert_eq!(render("---\n").to_canonical_string(), "null");
    }

    #[test]
    fn json_renderer_resolves_aliases_in_source_order() {
        let doc = YamlDoc::parse("a: &a 1\nb: *a\n").expect("valid YAML");
        assert_eq!(
            graph_to_json(&doc)
                .expect("aliases should render")
                .to_canonical_string(),
            "{\"a\":1,\"b\":1}"
        );
    }

    #[test]
    fn json_renderer_uses_latest_anchor_definition() {
        let doc = YamlDoc::parse("first: &a one\nsecond: *a\nthird: &a two\nfourth: *a\n")
            .expect("valid YAML");
        assert_eq!(
            graph_to_json(&doc)
                .expect("aliases should render")
                .to_canonical_string(),
            "{\"first\":\"one\",\"fourth\":\"two\",\"second\":\"one\",\"third\":\"two\"}"
        );
    }

    #[test]
    fn json_renderer_keeps_quoted_and_block_empty_scalars_as_strings() {
        assert_eq!(render("\"\"\n").to_canonical_string(), "\"\"");
        assert_eq!(render("|-\n").to_canonical_string(), "\"\"");
    }

    #[test]
    fn json_parser_normalizes_fixture_whitespace() {
        let parsed = JsonParser::new("{\n  \"a\": [true, null, \"x\"]\n}\n")
            .parse_single()
            .expect("valid JSON");
        assert_eq!(parsed.to_canonical_string(), "{\"a\":[true,null,\"x\"]}");
    }

    #[test]
    fn json_parser_accepts_multiple_stream_values() {
        let parsed = JsonParser::new("{\"a\":\"b\"}\n[\"c\"]\n\"d\"")
            .parse()
            .expect("valid JSON stream fixture");
        assert_eq!(
            parsed.to_canonical_string(),
            "{\"a\":\"b\"}\n[\"c\"]\n\"d\""
        );
    }

    #[test]
    fn json_parser_accepts_empty_stream_fixture() {
        let parsed = JsonParser::new("").parse().expect("empty stream fixture");
        assert_eq!(parsed.to_canonical_string(), "");
    }
}
