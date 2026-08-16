use std::io::Write;

use serde::{Serialize, ser};

use crate::value::{Mapping, Number, Tag, TaggedValue, Value};
use crate::{Error, Result};

/// Serializes a value to a UTF-8 YAML string.
///
/// # Errors
///
/// Returns an error when `value` cannot be represented as YAML.
pub fn to_string<T>(value: &T) -> Result<String>
where
    T: ?Sized + Serialize,
{
    let mut output = Vec::new();
    to_writer(&mut output, value)?;
    String::from_utf8(output).map_err(|error| Error::message(error.to_string()))
}

/// Serializes a value as one YAML document.
///
/// # Errors
///
/// Returns an error when serialization or writing to `writer` fails.
pub fn to_writer<W, T>(writer: W, value: &T) -> Result<()>
where
    W: Write,
    T: ?Sized + Serialize,
{
    let mut serializer = Serializer::new(writer);
    value.serialize(&mut serializer)
}

/// A YAML serializer writing one or more documents to an `io::Write` sink.
pub struct Serializer<W> {
    writer: W,
    documents: usize,
}

impl<W> Serializer<W>
where
    W: Write,
{
    /// Creates a serializer around `writer`.
    pub const fn new(writer: W) -> Self {
        Self {
            writer,
            documents: 0,
        }
    }

    /// Flushes the underlying writer.
    ///
    /// # Errors
    ///
    /// Returns an error when the underlying writer cannot be flushed.
    pub fn flush(&mut self) -> Result<()> {
        self.writer.flush().map_err(Error::io)
    }

    /// Flushes and returns the underlying writer.
    ///
    /// # Errors
    ///
    /// Returns an error when the underlying writer cannot be flushed.
    pub fn into_inner(mut self) -> Result<W> {
        self.flush()?;
        Ok(self.writer)
    }

    fn write_document(&mut self, value: &Value) -> Result<()> {
        if self.documents > 0 {
            self.writer.write_all(b"---\n").map_err(Error::io)?;
        }
        let mut output = String::new();
        render_value(value, 0, &mut output);
        if !output.ends_with('\n') {
            output.push('\n');
        }
        self.writer
            .write_all(output.as_bytes())
            .map_err(Error::io)?;
        self.documents += 1;
        Ok(())
    }

    fn collect<T>(&mut self, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        let value = crate::to_value(value)?;
        self.write_document(&value)
    }
}

pub struct ValueSequence {
    values: Vec<Value>,
    tag: Option<String>,
}

impl ValueSequence {
    fn new(len: Option<usize>, tag: Option<String>) -> Self {
        Self {
            values: Vec::with_capacity(len.unwrap_or(0)),
            tag,
        }
    }

    fn push<T>(&mut self, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        self.values.push(crate::to_value(value)?);
        Ok(())
    }

    fn finish(self) -> Value {
        let value = Value::Sequence(self.values);
        match self.tag {
            Some(tag) => Value::Tagged(Box::new(TaggedValue {
                tag: Tag::new(tag),
                value,
            })),
            None => value,
        }
    }
}

impl ser::SerializeSeq for ValueSequence {
    type Ok = Value;
    type Error = Error;
    fn serialize_element<T>(&mut self, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        self.push(value)
    }
    fn end(self) -> Result<Value> {
        Ok(self.finish())
    }
}
impl ser::SerializeTuple for ValueSequence {
    type Ok = Value;
    type Error = Error;
    fn serialize_element<T>(&mut self, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        self.push(value)
    }
    fn end(self) -> Result<Value> {
        Ok(self.finish())
    }
}
impl ser::SerializeTupleStruct for ValueSequence {
    type Ok = Value;
    type Error = Error;
    fn serialize_field<T>(&mut self, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        self.push(value)
    }
    fn end(self) -> Result<Value> {
        Ok(self.finish())
    }
}
impl ser::SerializeTupleVariant for ValueSequence {
    type Ok = Value;
    type Error = Error;
    fn serialize_field<T>(&mut self, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        self.push(value)
    }
    fn end(self) -> Result<Value> {
        Ok(self.finish())
    }
}

pub struct ValueMapping {
    entries: Mapping,
    pending: Option<Value>,
    tag: Option<String>,
}

impl ValueMapping {
    fn new(len: Option<usize>, tag: Option<String>) -> Self {
        Self {
            entries: Mapping::with_capacity(len.unwrap_or(0)),
            pending: None,
            tag,
        }
    }

    fn finish(self) -> Result<Value> {
        if self.pending.is_some() {
            return Err(Error::message("map ended before serializing a value"));
        }
        let value = Value::Mapping(self.entries);
        Ok(match self.tag {
            Some(tag) => Value::Tagged(Box::new(TaggedValue {
                tag: Tag::new(tag),
                value,
            })),
            None => value,
        })
    }
}

impl ser::SerializeMap for ValueMapping {
    type Ok = Value;
    type Error = Error;
    fn serialize_key<T>(&mut self, key: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        if self.pending.is_some() {
            return Err(Error::message("map key serialized before its value"));
        }
        self.pending = Some(crate::to_value(key)?);
        Ok(())
    }
    fn serialize_value<T>(&mut self, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        let key = self
            .pending
            .take()
            .ok_or_else(|| Error::message("map value serialized before its key"))?;
        self.entries.insert(key, crate::to_value(value)?);
        Ok(())
    }
    fn serialize_entry<K, V>(&mut self, key: &K, value: &V) -> Result<()>
    where
        K: ?Sized + Serialize,
        V: ?Sized + Serialize,
    {
        self.entries
            .insert(crate::to_value(key)?, crate::to_value(value)?);
        Ok(())
    }
    fn end(self) -> Result<Value> {
        self.finish()
    }
}

impl ser::SerializeStruct for ValueMapping {
    type Ok = Value;
    type Error = Error;
    fn serialize_field<T>(&mut self, key: &'static str, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        self.entries
            .insert(Value::String(key.to_owned()), crate::to_value(value)?);
        Ok(())
    }
    fn end(self) -> Result<Value> {
        self.finish()
    }
}

impl ser::SerializeStructVariant for ValueMapping {
    type Ok = Value;
    type Error = Error;
    fn serialize_field<T>(&mut self, key: &'static str, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        self.entries
            .insert(Value::String(key.to_owned()), crate::to_value(value)?);
        Ok(())
    }
    fn end(self) -> Result<Value> {
        self.finish()
    }
}

pub enum DocumentSequence<'a, W> {
    Sequence {
        serializer: &'a mut Serializer<W>,
        values: ValueSequence,
    },
    Mapping {
        serializer: &'a mut Serializer<W>,
        values: ValueMapping,
    },
}

impl<'a, W: Write> DocumentSequence<'a, W> {
    fn sequence(
        serializer: &'a mut Serializer<W>,
        len: Option<usize>,
        tag: Option<String>,
    ) -> Self {
        Self::Sequence {
            serializer,
            values: ValueSequence::new(len, tag),
        }
    }
    fn mapping(serializer: &'a mut Serializer<W>, len: Option<usize>, tag: Option<String>) -> Self {
        Self::Mapping {
            serializer,
            values: ValueMapping::new(len, tag),
        }
    }
}

impl<W: Write> ser::SerializeSeq for DocumentSequence<'_, W> {
    type Ok = ();
    type Error = Error;
    fn serialize_element<T>(&mut self, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        match self {
            Self::Sequence { values, .. } => values.push(value),
            Self::Mapping { .. } => unreachable!(),
        }
    }
    fn end(self) -> Result<()> {
        match self {
            Self::Sequence { serializer, values } => serializer.write_document(&values.finish()),
            Self::Mapping { .. } => unreachable!(),
        }
    }
}
impl<W: Write> ser::SerializeTuple for DocumentSequence<'_, W> {
    type Ok = ();
    type Error = Error;
    fn serialize_element<T>(&mut self, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        ser::SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> Result<()> {
        ser::SerializeSeq::end(self)
    }
}
impl<W: Write> ser::SerializeTupleStruct for DocumentSequence<'_, W> {
    type Ok = ();
    type Error = Error;
    fn serialize_field<T>(&mut self, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        ser::SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> Result<()> {
        ser::SerializeSeq::end(self)
    }
}
impl<W: Write> ser::SerializeTupleVariant for DocumentSequence<'_, W> {
    type Ok = ();
    type Error = Error;
    fn serialize_field<T>(&mut self, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        ser::SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> Result<()> {
        ser::SerializeSeq::end(self)
    }
}
impl<W: Write> ser::SerializeMap for DocumentSequence<'_, W> {
    type Ok = ();
    type Error = Error;
    fn serialize_key<T>(&mut self, key: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        match self {
            Self::Mapping { values, .. } => ser::SerializeMap::serialize_key(values, key),
            Self::Sequence { .. } => unreachable!(),
        }
    }
    fn serialize_value<T>(&mut self, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        match self {
            Self::Mapping { values, .. } => ser::SerializeMap::serialize_value(values, value),
            Self::Sequence { .. } => unreachable!(),
        }
    }
    fn serialize_entry<K, V>(&mut self, key: &K, value: &V) -> Result<()>
    where
        K: ?Sized + Serialize,
        V: ?Sized + Serialize,
    {
        match self {
            Self::Mapping { values, .. } => ser::SerializeMap::serialize_entry(values, key, value),
            Self::Sequence { .. } => unreachable!(),
        }
    }
    fn end(self) -> Result<()> {
        match self {
            Self::Mapping { serializer, values } => serializer.write_document(&values.finish()?),
            Self::Sequence { .. } => unreachable!(),
        }
    }
}
impl<W: Write> ser::SerializeStruct for DocumentSequence<'_, W> {
    type Ok = ();
    type Error = Error;
    fn serialize_field<T>(&mut self, key: &'static str, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        match self {
            Self::Mapping { values, .. } => {
                ser::SerializeStruct::serialize_field(values, key, value)
            }
            Self::Sequence { .. } => unreachable!(),
        }
    }
    fn end(self) -> Result<()> {
        ser::SerializeMap::end(self)
    }
}
impl<W: Write> ser::SerializeStructVariant for DocumentSequence<'_, W> {
    type Ok = ();
    type Error = Error;
    fn serialize_field<T>(&mut self, key: &'static str, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        match self {
            Self::Mapping { values, .. } => {
                ser::SerializeStruct::serialize_field(values, key, value)
            }
            Self::Sequence { .. } => unreachable!(),
        }
    }
    fn end(self) -> Result<()> {
        ser::SerializeMap::end(self)
    }
}

impl<'a, W> ser::Serializer for &'a mut Serializer<W>
where
    W: Write,
{
    type Ok = ();
    type Error = Error;
    type SerializeSeq = DocumentSequence<'a, W>;
    type SerializeTuple = DocumentSequence<'a, W>;
    type SerializeTupleStruct = DocumentSequence<'a, W>;
    type SerializeTupleVariant = DocumentSequence<'a, W>;
    type SerializeMap = DocumentSequence<'a, W>;
    type SerializeStruct = DocumentSequence<'a, W>;
    type SerializeStructVariant = DocumentSequence<'a, W>;

    fn serialize_bool(self, value: bool) -> Result<()> {
        self.write_document(&Value::Bool(value))
    }
    fn serialize_i8(self, value: i8) -> Result<()> {
        self.serialize_i128(value.into())
    }
    fn serialize_i16(self, value: i16) -> Result<()> {
        self.serialize_i128(value.into())
    }
    fn serialize_i32(self, value: i32) -> Result<()> {
        self.serialize_i128(value.into())
    }
    fn serialize_i64(self, value: i64) -> Result<()> {
        self.serialize_i128(value.into())
    }
    fn serialize_i128(self, value: i128) -> Result<()> {
        self.write_document(&Value::Number(Number::from(value)))
    }
    fn serialize_u8(self, value: u8) -> Result<()> {
        self.serialize_u128(value.into())
    }
    fn serialize_u16(self, value: u16) -> Result<()> {
        self.serialize_u128(value.into())
    }
    fn serialize_u32(self, value: u32) -> Result<()> {
        self.serialize_u128(value.into())
    }
    fn serialize_u64(self, value: u64) -> Result<()> {
        self.serialize_u128(value.into())
    }
    fn serialize_u128(self, value: u128) -> Result<()> {
        self.write_document(&Value::Number(Number::from(value)))
    }
    fn serialize_f32(self, value: f32) -> Result<()> {
        self.write_document(&Value::Number(Number::from(value)))
    }
    fn serialize_f64(self, value: f64) -> Result<()> {
        self.write_document(&Value::Number(Number::from(value)))
    }
    fn serialize_char(self, value: char) -> Result<()> {
        self.write_document(&Value::String(value.to_string()))
    }
    fn serialize_str(self, value: &str) -> Result<()> {
        self.write_document(&Value::String(value.to_owned()))
    }
    fn serialize_bytes(self, _value: &[u8]) -> Result<()> {
        Err(Error::message(
            "serialization and deserialization of bytes in YAML is not implemented",
        ))
    }
    fn serialize_none(self) -> Result<()> {
        self.write_document(&Value::Null)
    }
    fn serialize_some<T>(self, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        self.collect(value)
    }
    fn serialize_unit(self) -> Result<()> {
        self.write_document(&Value::Null)
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<()> {
        self.serialize_unit()
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> Result<()> {
        self.write_document(&Value::String(variant.to_owned()))
    }
    fn serialize_newtype_struct<T>(self, name: &'static str, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        let value =
            ser::Serializer::serialize_newtype_struct(crate::value::Serializer, name, value)?;
        self.write_document(&value)
    }
    fn serialize_newtype_variant<T>(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        let value = crate::to_value(value)?;
        if matches!(value, Value::Tagged(..)) {
            return Err(Error::message(
                "serializing nested enums in YAML is not supported",
            ));
        }
        self.write_document(&Value::Tagged(Box::new(TaggedValue {
            tag: Tag::new(variant),
            value,
        })))
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<Self::SerializeSeq> {
        Ok(DocumentSequence::sequence(self, len, None))
    }
    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple> {
        Ok(DocumentSequence::sequence(self, Some(len), None))
    }
    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleStruct> {
        Ok(DocumentSequence::sequence(self, Some(len), None))
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleVariant> {
        Ok(DocumentSequence::sequence(
            self,
            Some(len),
            Some(variant.to_owned()),
        ))
    }
    fn serialize_map(self, len: Option<usize>) -> Result<Self::SerializeMap> {
        Ok(DocumentSequence::mapping(self, len, None))
    }
    fn serialize_struct(self, _name: &'static str, len: usize) -> Result<Self::SerializeStruct> {
        Ok(DocumentSequence::mapping(self, Some(len), None))
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStructVariant> {
        Ok(DocumentSequence::mapping(
            self,
            Some(len),
            Some(variant.to_owned()),
        ))
    }
    fn collect_str<T>(self, value: &T) -> Result<()>
    where
        T: ?Sized + std::fmt::Display,
    {
        self.serialize_str(&value.to_string())
    }
    fn is_human_readable(&self) -> bool {
        true
    }
}

fn render_value(value: &Value, indent: usize, output: &mut String) {
    enum RenderAction<'a> {
        Value(&'a Value, usize),
        Nested(&'a Value, usize),
        Inline(&'a Value),
        Indent(usize),
        Text(&'static str),
    }

    let mut pending = vec![RenderAction::Value(value, indent)];
    while let Some(action) = pending.pop() {
        match action {
            RenderAction::Text(text) => output.push_str(text),
            RenderAction::Indent(indent) => push_indent(output, indent),
            RenderAction::Inline(value) => render_inline(value, output),
            RenderAction::Nested(value, indent) => match value {
                Value::Tagged(tagged) if !is_inline(&tagged.value) => {
                    output.push(' ');
                    output.push('!');
                    output.push_str(tagged.tag.as_suffix());
                    output.push('\n');
                    pending.push(RenderAction::Value(&tagged.value, indent));
                }
                _ if is_inline(value) => {
                    output.push(' ');
                    pending.push(RenderAction::Inline(value));
                }
                _ => {
                    output.push('\n');
                    pending.push(RenderAction::Value(value, indent));
                }
            },
            RenderAction::Value(value, indent) => match value {
                Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {
                    push_indent(output, indent);
                    render_scalar(value, output);
                }
                Value::Sequence(values) if values.is_empty() => {
                    push_indent(output, indent);
                    output.push_str("[]");
                }
                Value::Sequence(values) => {
                    for (index, value) in values.iter().enumerate().rev() {
                        pending.push(RenderAction::Nested(value, indent + 2));
                        pending.push(RenderAction::Text("-"));
                        pending.push(RenderAction::Indent(indent));
                        if index > 0 {
                            pending.push(RenderAction::Text("\n"));
                        }
                    }
                }
                Value::Mapping(entries) if entries.is_empty() => {
                    push_indent(output, indent);
                    output.push_str("{}");
                }
                Value::Mapping(entries) => {
                    for (index, (key, value)) in entries.iter().enumerate().rev() {
                        pending.push(RenderAction::Nested(value, indent + 2));
                        if is_inline(key) {
                            pending.push(RenderAction::Text(":"));
                            pending.push(RenderAction::Inline(key));
                            pending.push(RenderAction::Indent(indent));
                        } else {
                            pending.push(RenderAction::Text(":"));
                            pending.push(RenderAction::Indent(indent));
                            pending.push(RenderAction::Text("\n"));
                            pending.push(RenderAction::Nested(key, indent + 2));
                            pending.push(RenderAction::Text("?"));
                            pending.push(RenderAction::Indent(indent));
                        }
                        if index > 0 {
                            pending.push(RenderAction::Text("\n"));
                        }
                    }
                }
                Value::Tagged(tagged) => {
                    push_indent(output, indent);
                    output.push('!');
                    output.push_str(tagged.tag.as_suffix());
                    if is_inline(&tagged.value) {
                        output.push(' ');
                        pending.push(RenderAction::Inline(&tagged.value));
                    } else {
                        output.push('\n');
                        pending.push(RenderAction::Value(&tagged.value, indent));
                    }
                }
            },
        }
    }
}

fn is_inline(value: &Value) -> bool {
    let mut value = value;
    while let Value::Tagged(tagged) = value {
        value = &tagged.value;
    }
    matches!(
        value,
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)
    ) || matches!(value, Value::Sequence(values) if values.is_empty())
        || matches!(value, Value::Mapping(entries) if entries.is_empty())
}

fn render_inline(mut value: &Value, output: &mut String) {
    while let Value::Tagged(tagged) = value {
        output.push('!');
        output.push_str(tagged.tag.as_suffix());
        output.push(' ');
        value = &tagged.value;
    }
    match value {
        Value::Sequence(values) if values.is_empty() => output.push_str("[]"),
        Value::Mapping(entries) if entries.is_empty() => output.push_str("{}"),
        _ => render_scalar(value, output),
    }
}

fn render_scalar(value: &Value, output: &mut String) {
    match value {
        Value::Null => output.push_str("null"),
        Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        Value::Number(value) => output.push_str(&value.to_string()),
        Value::String(value) => render_string(value, output),
        _ => unreachable!("collections are not scalars"),
    }
}

fn render_string(value: &str, output: &mut String) {
    if is_safe_plain(value) {
        output.push_str(value);
        return;
    }
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '\u{08}' => output.push_str("\\b"),
            '\u{0C}' => output.push_str("\\f"),
            character if character.is_control() => {
                use std::fmt::Write as _;
                let _ = write!(output, "\\u{:04X}", character as u32);
            }
            character => output.push(character),
        }
    }
    output.push('"');
}

fn is_safe_plain(value: &str) -> bool {
    if value.is_empty() || value.trim() != value || value.contains(['\n', '\r', '\t']) {
        return false;
    }
    if value.starts_with([
        '-', '?', ':', ',', '[', ']', '{', '}', '#', '&', '*', '!', '|', '>', '\'', '"', '%', '@',
        '`',
    ]) {
        return false;
    }
    if value.contains(": ") || value.contains(" #") || value == "---" || value == "..." {
        return false;
    }
    if matches!(
        value,
        "~" | "null"
            | "Null"
            | "NULL"
            | "true"
            | "True"
            | "TRUE"
            | "false"
            | "False"
            | "FALSE"
            | ".inf"
            | ".Inf"
            | ".INF"
            | "-.inf"
            | "-.Inf"
            | "-.INF"
            | ".nan"
            | ".NaN"
            | ".NAN"
    ) {
        return false;
    }
    !looks_numeric(value)
}

fn looks_numeric(value: &str) -> bool {
    let value = value.replace('_', "");
    let unsigned = value.strip_prefix(['+', '-']).unwrap_or(&value);
    if unsigned
        .strip_prefix("0x")
        .is_some_and(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_hexdigit()))
    {
        return true;
    }
    if unsigned
        .strip_prefix("0o")
        .is_some_and(|v| !v.is_empty() && v.chars().all(|c| matches!(c, '0'..='7')))
    {
        return true;
    }
    if unsigned
        .strip_prefix("0b")
        .is_some_and(|v| !v.is_empty() && v.chars().all(|c| matches!(c, '0' | '1')))
    {
        return true;
    }
    value.parse::<i128>().is_ok() || value.parse::<u128>().is_ok() || value.parse::<f64>().is_ok()
}

fn push_indent(output: &mut String, indent: usize) {
    output.extend(std::iter::repeat_n(' ', indent));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renderer_handles_deep_sequences_iteratively() {
        let depth = 1024;
        let mut value = Value::String("value".to_owned());
        for _ in 0..depth {
            value = Value::Sequence(vec![value]);
        }

        let mut output = String::new();
        render_value(&value, 0, &mut output);
        assert_eq!(output.matches('-').count(), depth);
        assert!(output.ends_with(&format!("{}- value", "  ".repeat(depth - 1))));
    }

    #[test]
    fn renderer_handles_tag_chains_iteratively() {
        let mut value = Value::String("value".to_owned());
        for index in (0..1024).rev() {
            value = Value::Tagged(Box::new(TaggedValue {
                tag: Tag::new(format!("tag{index}")),
                value,
            }));
        }

        let mut output = String::new();
        render_value(&value, 0, &mut output);
        assert!(output.starts_with("!tag0 !tag1 !tag2 "));
        assert!(output.ends_with("!tag1023 value"));
    }
}
