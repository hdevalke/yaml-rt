#![allow(clippy::collapsible_if)]
use std::collections::BTreeMap;
use std::fmt::{self, Write};
use std::ops::Index;

use yaml_rt_core::YamlDoc;

use crate::{Error, model};

pub type Map = BTreeMap<String, Value>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Number(String);

impl From<i32> for Number {
    fn from(value: i32) -> Self {
        Self(value.to_string())
    }
}
impl Number {
    pub(crate) fn from_yaml(text: String) -> Self {
        Self(text)
    }
}

impl fmt::Display for Number {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(Number),
    String(String),
    Array(Vec<Value>),
    Object(Map),
}

impl Value {
    pub fn parse(source: &str) -> Result<Self, Error> {
        let normalized = decode_json_surrogates(source);
        let doc = YamlDoc::parse(&normalized).map_err(Error::source)?;
        if doc.document_count() != 1 {
            return Err(Error::new("JSON value must contain exactly one document"));
        }
        Ok(model::from_document(&doc, 0)?.value)
    }
    pub fn as_object(&self) -> Option<&Map> {
        if let Self::Object(v) = self {
            Some(v)
        } else {
            None
        }
    }
    pub fn as_array(&self) -> Option<&Vec<Value>> {
        if let Self::Array(v) = self {
            Some(v)
        } else {
            None
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        if let Self::String(v) = self {
            Some(v)
        } else {
            None
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        if let Self::Bool(v) = self {
            Some(*v)
        } else {
            None
        }
    }
    pub fn as_number(&self) -> Option<&Number> {
        if let Self::Number(v) = self {
            Some(v)
        } else {
            None
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        self.as_number()?.0.parse().ok()
    }
    pub fn as_u64(&self) -> Option<u64> {
        self.as_number()?.0.parse().ok()
    }
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }
    pub fn is_boolean(&self) -> bool {
        matches!(self, Self::Bool(_))
    }
    pub fn is_number(&self) -> bool {
        matches!(self, Self::Number(_))
    }
    pub fn is_string(&self) -> bool {
        matches!(self, Self::String(_))
    }
    pub fn is_array(&self) -> bool {
        matches!(self, Self::Array(_))
    }
    pub fn is_object(&self) -> bool {
        matches!(self, Self::Object(_))
    }
    pub fn get(&self, key: &str) -> Option<&Self> {
        self.as_object()?.get(key)
    }
    pub fn pointer(&self, pointer: &str) -> Option<&Self> {
        if pointer.is_empty() {
            return Some(self);
        }
        let mut value = self;
        for part in pointer.strip_prefix('/')?.split('/') {
            let key = part.replace("~1", "/").replace("~0", "~");
            value = match value {
                Self::Object(map) => map.get(&key)?,
                Self::Array(items) => {
                    let bytes = key.as_bytes();
                    if bytes.is_empty()
                        || !bytes.iter().all(u8::is_ascii_digit)
                        || bytes.len() > 1 && bytes[0] == b'0'
                    {
                        return None;
                    }
                    items.get(key.parse::<usize>().ok()?)?
                }
                _ => return None,
            };
        }
        Some(value)
    }
    pub fn to_pretty_string(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, 0, true).unwrap();
        out
    }
    fn write(&self, out: &mut String, depth: usize, pretty: bool) -> fmt::Result {
        match self {
            Self::Null => out.push_str("null"),
            Self::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            Self::Number(value) => out.push_str(&value.0),
            Self::String(value) => write_string(out, value)?,
            Self::Array(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    if pretty {
                        out.push('\n');
                        indent(out, depth + 1);
                    }
                    item.write(out, depth + 1, pretty)?;
                }
                if pretty && !items.is_empty() {
                    out.push('\n');
                    indent(out, depth);
                }
                out.push(']');
            }
            Self::Object(items) => {
                out.push('{');
                for (index, (key, value)) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    if pretty {
                        out.push('\n');
                        indent(out, depth + 1);
                    }
                    write_string(out, key)?;
                    out.push(':');
                    if pretty {
                        out.push(' ');
                    }
                    value.write(out, depth + 1, pretty)?;
                }
                if pretty && !items.is_empty() {
                    out.push('\n');
                    indent(out, depth);
                }
                out.push('}');
            }
        }
        Ok(())
    }
}
impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut out = String::new();
        self.write(&mut out, 0, false)?;
        f.write_str(&out)
    }
}
impl Index<&str> for Value {
    type Output = Value;
    fn index(&self, key: &str) -> &Value {
        &self.as_object().expect("object")[key]
    }
}
fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}
fn write_string(out: &mut String, value: &str) -> fmt::Result {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if ch < ' ' => write!(out, "\\u{:04x}", ch as u32)?,
            ch => out.push(ch),
        }
    }
    out.push('"');
    Ok(())
}

// YAML requires Unicode scalar escapes; JSON permits UTF-16 surrogate escapes.
fn decode_json_surrogates(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out = String::with_capacity(source.len());
    let mut start = 0;
    let mut index = 0;
    let mut quoted = false;
    while index < bytes.len() {
        if bytes[index] == b'"' {
            quoted = !quoted;
            index += 1;
            continue;
        }
        if quoted && bytes[index] == b'\\' {
            if bytes.get(index + 1) == Some(&b'u') && index + 6 <= bytes.len() {
                if let Ok(high) = u16::from_str_radix(&source[index + 2..index + 6], 16) {
                    if (0xD800..=0xDBFF).contains(&high)
                        && bytes.get(index + 6..index + 8) == Some(&b"\\u"[..])
                        && index + 12 <= bytes.len()
                    {
                        if let Ok(low) = u16::from_str_radix(&source[index + 8..index + 12], 16) {
                            if (0xDC00..=0xDFFF).contains(&low) {
                                let scalar = 0x10000
                                    + ((high as u32 - 0xD800) << 10)
                                    + (low as u32 - 0xDC00);
                                out.push_str(&source[start..index]);
                                out.push(char::from_u32(scalar).unwrap());
                                index += 12;
                                start = index;
                                continue;
                            }
                        }
                    }
                    if (0xD800..=0xDFFF).contains(&high) {
                        out.push_str(&source[start..index]);
                        out.push('\u{FFFD}');
                        index += 6;
                        start = index;
                        continue;
                    }
                }
            }
            index += 2;
            continue;
        }
        index += 1;
    }
    out.push_str(&source[start..]);
    out
}
