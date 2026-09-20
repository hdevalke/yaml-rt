//! JSON Schema validation and schema inference over yaml-rt documents.

mod generate;
mod model;
mod numeric;
mod validate;
mod value;

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

pub use value::Value;
use yaml_rt_core::{JsonPointer, Span, YamlDoc};

pub use generate::generate_schema;

const DIALECT: &str = "https://json-schema.org/draft/2020-12/schema";

/// A schema loading, instance conversion, or validation error.
#[derive(Debug, Clone)]
pub struct Error {
    message: String,
    instance_path: Option<String>,
    schema_path: Option<String>,
    source_span: Option<Span>,
}

impl Error {
    fn source(error: impl fmt::Display) -> Self {
        Self::new(error.to_string())
    }
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            instance_path: None,
            schema_path: None,
            source_span: None,
        }
    }
    fn instance(path: impl Into<String>, message: impl Into<String>, span: Option<Span>) -> Self {
        Self {
            message: message.into(),
            instance_path: Some(path.into()),
            schema_path: None,
            source_span: span,
        }
    }
    fn keyword(
        path: impl Into<String>,
        schema: impl Into<String>,
        message: impl Into<String>,
        span: Option<Span>,
    ) -> Self {
        Self {
            message: message.into(),
            instance_path: Some(path.into()),
            schema_path: Some(schema.into()),
            source_span: span,
        }
    }
    /// JSON Pointer to the failing instance value, if available.
    pub fn instance_path(&self) -> Option<&str> {
        self.instance_path.as_deref()
    }
    /// JSON Pointer to the failing schema keyword, if available.
    pub fn schema_path(&self) -> Option<&str> {
        self.schema_path.as_deref()
    }
    /// Span of the failing YAML value, if available.
    pub fn source_span(&self) -> Option<Span> {
        self.source_span
    }
    /// Primary error message without location metadata.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(path) = &self.instance_path {
            write!(f, "at instance {path:?}: ")?;
        }
        write!(f, "{}", self.message)?;
        if let Some(path) = &self.schema_path {
            write!(f, " (schema {path})")?;
        }
        Ok(())
    }
}
impl std::error::Error for Error {}

/// Parsed JSON Schema reusable across YAML documents.
pub struct Schema {
    root: Value,
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    origin: Option<PathBuf>,
    format_assertion: bool,
    resources: std::collections::HashMap<String, Value>,
}

impl Schema {
    /// Parses a JSON or YAML schema. `base_path` locates relative file references.
    pub fn parse(source: &str, base_path: Option<&Path>) -> Result<Self, Error> {
        let root = parse_schema(source, base_path)?;
        let attach_source = |mut error: Error| {
            if let Ok(doc) = YamlDoc::parse(source)
                && let Ok(instance) = model::from_document(&doc, 0)
            {
                error.source_span = error
                    .schema_path
                    .as_ref()
                    .and_then(|path| instance.spans.get(path).copied());
            }
            error
        };
        validate::check_schema(&root, "").map_err(&attach_source)?;
        validate::check_meta_schema(&root).map_err(&attach_source)?;
        let origin = base_path.map(Path::to_path_buf);
        Ok(Self {
            root,
            origin,
            format_assertion: false,
            resources: Default::default(),
        })
    }

    /// Reads and compiles a JSON or YAML schema file.
    pub fn from_path(path: &Path) -> Result<Self, Error> {
        let source = fs::read_to_string(path).map_err(Error::source)?;
        Self::parse(&source, Some(path))
    }

    /// Validates one document in a parsed YAML stream.
    pub fn validate(&self, doc: &YamlDoc, document: usize) -> Result<(), Error> {
        let instance = model::from_document(doc, document)?;
        validate::validate(self, &instance)
    }

    /// Validates one selected YAML value while retaining its source spans.
    pub fn validate_pointer(
        &self,
        doc: &YamlDoc,
        document: usize,
        pointer: &JsonPointer,
    ) -> Result<(), Error> {
        let node = doc
            .resolve_pointer(document, pointer)
            .map_err(Error::source)?;
        let instance = model::from_node(doc, Some(node))?;
        validate::validate(self, &instance)
    }

    /// Validates an already parsed JSON instance.
    pub fn validate_json(&self, value: &Value) -> Result<(), Error> {
        validate::validate(
            self,
            &model::Instance {
                value: value.clone(),
                spans: Default::default(),
            },
        )
    }

    /// Enables or disables assertion checks for standard `format` names.
    pub fn with_format_assertions(mut self, enabled: bool) -> Self {
        self.format_assertion = enabled;
        self
    }

    /// Registers an already available schema resource under an absolute URI.
    /// This does not enable network access.
    pub fn register_resource(&mut self, uri: &str, source: &str) -> Result<(), Error> {
        let url = url::Url::parse(uri).map_err(Error::source)?;
        let value = Value::parse(source)?;
        validate::check_schema(&value, "")?;
        validate::check_meta_schema(&value)?;
        self.resources.insert(url.to_string(), value);
        Ok(())
    }
}

fn parse_schema(source: &str, path: Option<&Path>) -> Result<Value, Error> {
    if path.is_some_and(|path| {
        path.extension()
            .is_some_and(|extension| extension == "json")
    }) {
        return Value::parse(source);
    }
    let doc = YamlDoc::parse(source).map_err(|error| {
        let span = error.diagnostic.span;
        let mut result = Error::source(error);
        result.source_span = Some(span);
        result
    })?;
    if doc.document_count() != 1 {
        return Err(Error::new("schema must contain exactly one document"));
    }
    Ok(model::from_document(&doc, 0)?.value)
}
