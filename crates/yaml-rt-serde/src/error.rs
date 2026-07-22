use std::{fmt, io};

use yaml_rt_core::{LineCol, Span, YamlDoc, YamlError};

/// Result type used by this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Location of a deserialization error in the YAML input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Location {
    index: usize,
    line: usize,
    column: usize,
}

impl Location {
    /// Zero-based byte index.
    #[must_use]
    pub const fn index(&self) -> usize {
        self.index
    }

    /// One-based line number.
    #[must_use]
    pub const fn line(&self) -> usize {
        self.line
    }

    /// One-based byte column.
    #[must_use]
    pub const fn column(&self) -> usize {
        self.column
    }
}

/// Error produced while parsing, deserializing, serializing, or writing YAML.
#[derive(Debug)]
pub struct Error {
    pub(crate) message: String,
    pub(crate) path: Option<String>,
    pub(crate) location: Option<Location>,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl Error {
    pub(crate) fn message(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            path: None,
            location: None,
            source: None,
        }
    }

    pub(crate) fn at(mut self, doc: &YamlDoc, span: Span) -> Self {
        if self.location.is_none() {
            let LineCol { line, column } = doc.source().line_col(span.start as usize);
            self.location = Some(Location {
                index: span.start as usize,
                line,
                column,
            });
        }
        self
    }

    pub(crate) fn with_path(mut self, path: &str) -> Self {
        if path != "." && self.path.is_none() {
            self.path = Some(path.to_owned());
        }
        self
    }

    pub(crate) fn io(error: io::Error) -> Self {
        Self {
            message: error.to_string(),
            path: None,
            location: None,
            source: Some(Box::new(error)),
        }
    }

    /// Returns the input location associated with this error, when available.
    #[must_use]
    pub const fn location(&self) -> Option<Location> {
        self.location
    }
}

impl Clone for Error {
    fn clone(&self) -> Self {
        Self {
            message: self.message.clone(),
            path: self.path.clone(),
            location: self.location,
            source: None,
        }
    }
}

impl From<YamlError> for Error {
    fn from(error: YamlError) -> Self {
        let location = error.diagnostic.position.map(|position| Location {
            index: error.diagnostic.span.start as usize,
            line: position.line,
            column: position.column,
        });
        Self {
            message: error.to_string(),
            path: None,
            location,
            source: Some(Box::new(error)),
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::io(error)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(path) = &self.path {
            write!(formatter, "{path}: ")?;
        }
        formatter.write_str(&self.message)?;
        if let Some(location) = self.location {
            write!(
                formatter,
                " at line {} column {}",
                location.line, location.column
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn std::error::Error + 'static))
    }
}

impl serde::de::Error for Error {
    fn custom<T>(message: T) -> Self
    where
        T: fmt::Display,
    {
        Self::message(message.to_string())
    }
}

impl serde::ser::Error for Error {
    fn custom<T>(message: T) -> Self
    where
        T: fmt::Display,
    {
        Self::message(message.to_string())
    }
}
