use std::io::Write;

use serde::{Serialize, ser};

use crate::{Error, Result};

/// Serializes a value to a UTF-8 YAML string.
pub fn to_string<T>(value: &T) -> Result<String>
where
    T: ?Sized + Serialize,
{
    let mut output = Vec::new();
    to_writer(&mut output, value)?;
    String::from_utf8(output).map_err(|error| Error::message(error.to_string()))
}

/// Serializes a value as one YAML document.
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
    pub fn flush(&mut self) -> Result<()> {
        self.writer.flush().map_err(Error::io)
    }

    /// Flushes and returns the underlying writer.
    pub fn into_inner(mut self) -> Result<W> {
        self.flush()?;
        Ok(self.writer)
    }

    fn write_document(&mut self, value: SerValue) -> Result<()> {
        if self.documents > 0 {
            self.writer.write_all(b"---\n").map_err(Error::io)?;
        }
        let mut output = String::new();
        render_value(&value, 0, &mut output);
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
        self.write_document(value.serialize(ValueSerializer)?)
    }
}

#[derive(Debug)]
pub enum SerValue {
    Null,
    Bool(bool),
    Signed(i128),
    Unsigned(u128),
    Float(f64),
    String(String),
    Sequence(Vec<SerValue>),
    Mapping(Vec<(SerValue, SerValue)>),
    Tagged(String, Box<SerValue>),
}

struct ValueSerializer;

impl ser::Serializer for ValueSerializer {
    type Ok = SerValue;
    type Error = Error;
    type SerializeSeq = ValueSequence;
    type SerializeTuple = ValueSequence;
    type SerializeTupleStruct = ValueSequence;
    type SerializeTupleVariant = ValueSequence;
    type SerializeMap = ValueMapping;
    type SerializeStruct = ValueMapping;
    type SerializeStructVariant = ValueMapping;

    fn serialize_bool(self, value: bool) -> Result<SerValue> {
        Ok(SerValue::Bool(value))
    }
    fn serialize_i8(self, value: i8) -> Result<SerValue> {
        self.serialize_i128(value.into())
    }
    fn serialize_i16(self, value: i16) -> Result<SerValue> {
        self.serialize_i128(value.into())
    }
    fn serialize_i32(self, value: i32) -> Result<SerValue> {
        self.serialize_i128(value.into())
    }
    fn serialize_i64(self, value: i64) -> Result<SerValue> {
        self.serialize_i128(value.into())
    }
    fn serialize_i128(self, value: i128) -> Result<SerValue> {
        Ok(SerValue::Signed(value))
    }
    fn serialize_u8(self, value: u8) -> Result<SerValue> {
        self.serialize_u128(value.into())
    }
    fn serialize_u16(self, value: u16) -> Result<SerValue> {
        self.serialize_u128(value.into())
    }
    fn serialize_u32(self, value: u32) -> Result<SerValue> {
        self.serialize_u128(value.into())
    }
    fn serialize_u64(self, value: u64) -> Result<SerValue> {
        self.serialize_u128(value.into())
    }
    fn serialize_u128(self, value: u128) -> Result<SerValue> {
        Ok(SerValue::Unsigned(value))
    }
    fn serialize_f32(self, value: f32) -> Result<SerValue> {
        Ok(SerValue::Float(value.into()))
    }
    fn serialize_f64(self, value: f64) -> Result<SerValue> {
        Ok(SerValue::Float(value))
    }
    fn serialize_char(self, value: char) -> Result<SerValue> {
        Ok(SerValue::String(value.to_string()))
    }
    fn serialize_str(self, value: &str) -> Result<SerValue> {
        Ok(SerValue::String(value.to_owned()))
    }
    fn serialize_bytes(self, _value: &[u8]) -> Result<SerValue> {
        Err(Error::message(
            "serialization and deserialization of bytes in YAML is not implemented",
        ))
    }
    fn serialize_none(self) -> Result<SerValue> {
        Ok(SerValue::Null)
    }
    fn serialize_some<T>(self, value: &T) -> Result<SerValue>
    where
        T: ?Sized + Serialize,
    {
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<SerValue> {
        Ok(SerValue::Null)
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<SerValue> {
        Ok(SerValue::Null)
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> Result<SerValue> {
        Ok(SerValue::String(variant.to_owned()))
    }
    fn serialize_newtype_struct<T>(self, _name: &'static str, value: &T) -> Result<SerValue>
    where
        T: ?Sized + Serialize,
    {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T>(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<SerValue>
    where
        T: ?Sized + Serialize,
    {
        let value = value.serialize(self)?;
        if matches!(value, SerValue::Tagged(..)) {
            return Err(Error::message(
                "serializing nested enums in YAML is not supported",
            ));
        }
        Ok(SerValue::Tagged(variant.to_owned(), Box::new(value)))
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<ValueSequence> {
        Ok(ValueSequence::new(len, None))
    }
    fn serialize_tuple(self, len: usize) -> Result<ValueSequence> {
        Ok(ValueSequence::new(Some(len), None))
    }
    fn serialize_tuple_struct(self, _name: &'static str, len: usize) -> Result<ValueSequence> {
        Ok(ValueSequence::new(Some(len), None))
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<ValueSequence> {
        Ok(ValueSequence::new(Some(len), Some(variant.to_owned())))
    }
    fn serialize_map(self, len: Option<usize>) -> Result<ValueMapping> {
        Ok(ValueMapping::new(len, None))
    }
    fn serialize_struct(self, _name: &'static str, len: usize) -> Result<ValueMapping> {
        Ok(ValueMapping::new(Some(len), None))
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<ValueMapping> {
        Ok(ValueMapping::new(Some(len), Some(variant.to_owned())))
    }
    fn collect_str<T>(self, value: &T) -> Result<SerValue>
    where
        T: ?Sized + std::fmt::Display,
    {
        Ok(SerValue::String(value.to_string()))
    }
    fn is_human_readable(&self) -> bool {
        true
    }
}

pub struct ValueSequence {
    values: Vec<SerValue>,
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
        self.values.push(value.serialize(ValueSerializer)?);
        Ok(())
    }

    fn finish(self) -> SerValue {
        let value = SerValue::Sequence(self.values);
        match self.tag {
            Some(tag) => SerValue::Tagged(tag, Box::new(value)),
            None => value,
        }
    }
}

impl ser::SerializeSeq for ValueSequence {
    type Ok = SerValue;
    type Error = Error;
    fn serialize_element<T>(&mut self, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        self.push(value)
    }
    fn end(self) -> Result<SerValue> {
        Ok(self.finish())
    }
}
impl ser::SerializeTuple for ValueSequence {
    type Ok = SerValue;
    type Error = Error;
    fn serialize_element<T>(&mut self, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        self.push(value)
    }
    fn end(self) -> Result<SerValue> {
        Ok(self.finish())
    }
}
impl ser::SerializeTupleStruct for ValueSequence {
    type Ok = SerValue;
    type Error = Error;
    fn serialize_field<T>(&mut self, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        self.push(value)
    }
    fn end(self) -> Result<SerValue> {
        Ok(self.finish())
    }
}
impl ser::SerializeTupleVariant for ValueSequence {
    type Ok = SerValue;
    type Error = Error;
    fn serialize_field<T>(&mut self, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        self.push(value)
    }
    fn end(self) -> Result<SerValue> {
        Ok(self.finish())
    }
}

pub struct ValueMapping {
    entries: Vec<(SerValue, SerValue)>,
    pending: Option<SerValue>,
    tag: Option<String>,
}

impl ValueMapping {
    fn new(len: Option<usize>, tag: Option<String>) -> Self {
        Self {
            entries: Vec::with_capacity(len.unwrap_or(0)),
            pending: None,
            tag,
        }
    }

    fn finish(self) -> Result<SerValue> {
        if self.pending.is_some() {
            return Err(Error::message("map ended before serializing a value"));
        }
        let value = SerValue::Mapping(self.entries);
        Ok(match self.tag {
            Some(tag) => SerValue::Tagged(tag, Box::new(value)),
            None => value,
        })
    }
}

impl ser::SerializeMap for ValueMapping {
    type Ok = SerValue;
    type Error = Error;
    fn serialize_key<T>(&mut self, key: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        if self.pending.is_some() {
            return Err(Error::message("map key serialized before its value"));
        }
        self.pending = Some(key.serialize(ValueSerializer)?);
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
        self.entries.push((key, value.serialize(ValueSerializer)?));
        Ok(())
    }
    fn serialize_entry<K, V>(&mut self, key: &K, value: &V) -> Result<()>
    where
        K: ?Sized + Serialize,
        V: ?Sized + Serialize,
    {
        self.entries.push((
            key.serialize(ValueSerializer)?,
            value.serialize(ValueSerializer)?,
        ));
        Ok(())
    }
    fn end(self) -> Result<SerValue> {
        self.finish()
    }
}

impl ser::SerializeStruct for ValueMapping {
    type Ok = SerValue;
    type Error = Error;
    fn serialize_field<T>(&mut self, key: &'static str, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        self.entries.push((
            SerValue::String(key.to_owned()),
            value.serialize(ValueSerializer)?,
        ));
        Ok(())
    }
    fn end(self) -> Result<SerValue> {
        self.finish()
    }
}

impl ser::SerializeStructVariant for ValueMapping {
    type Ok = SerValue;
    type Error = Error;
    fn serialize_field<T>(&mut self, key: &'static str, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        self.entries.push((
            SerValue::String(key.to_owned()),
            value.serialize(ValueSerializer)?,
        ));
        Ok(())
    }
    fn end(self) -> Result<SerValue> {
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
            _ => unreachable!(),
        }
    }
    fn end(self) -> Result<()> {
        match self {
            Self::Sequence { serializer, values } => serializer.write_document(values.finish()),
            _ => unreachable!(),
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
            _ => unreachable!(),
        }
    }
    fn serialize_value<T>(&mut self, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        match self {
            Self::Mapping { values, .. } => ser::SerializeMap::serialize_value(values, value),
            _ => unreachable!(),
        }
    }
    fn serialize_entry<K, V>(&mut self, key: &K, value: &V) -> Result<()>
    where
        K: ?Sized + Serialize,
        V: ?Sized + Serialize,
    {
        match self {
            Self::Mapping { values, .. } => ser::SerializeMap::serialize_entry(values, key, value),
            _ => unreachable!(),
        }
    }
    fn end(self) -> Result<()> {
        match self {
            Self::Mapping { serializer, values } => serializer.write_document(values.finish()?),
            _ => unreachable!(),
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
            _ => unreachable!(),
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
            _ => unreachable!(),
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
        self.write_document(SerValue::Bool(value))
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
        self.write_document(SerValue::Signed(value))
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
        self.write_document(SerValue::Unsigned(value))
    }
    fn serialize_f32(self, value: f32) -> Result<()> {
        self.write_document(SerValue::Float(value.into()))
    }
    fn serialize_f64(self, value: f64) -> Result<()> {
        self.write_document(SerValue::Float(value))
    }
    fn serialize_char(self, value: char) -> Result<()> {
        self.write_document(SerValue::String(value.to_string()))
    }
    fn serialize_str(self, value: &str) -> Result<()> {
        self.write_document(SerValue::String(value.to_owned()))
    }
    fn serialize_bytes(self, _value: &[u8]) -> Result<()> {
        Err(Error::message(
            "serialization and deserialization of bytes in YAML is not implemented",
        ))
    }
    fn serialize_none(self) -> Result<()> {
        self.write_document(SerValue::Null)
    }
    fn serialize_some<T>(self, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        self.collect(value)
    }
    fn serialize_unit(self) -> Result<()> {
        self.write_document(SerValue::Null)
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
        self.write_document(SerValue::String(variant.to_owned()))
    }
    fn serialize_newtype_struct<T>(self, _name: &'static str, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        self.collect(value)
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
        let value = value.serialize(ValueSerializer)?;
        if matches!(value, SerValue::Tagged(..)) {
            return Err(Error::message(
                "serializing nested enums in YAML is not supported",
            ));
        }
        self.write_document(SerValue::Tagged(variant.to_owned(), Box::new(value)))
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

fn render_value(value: &SerValue, indent: usize, output: &mut String) {
    match value {
        SerValue::Null
        | SerValue::Bool(_)
        | SerValue::Signed(_)
        | SerValue::Unsigned(_)
        | SerValue::Float(_)
        | SerValue::String(_) => {
            push_indent(output, indent);
            render_scalar(value, output);
        }
        SerValue::Sequence(values) => render_sequence(values, indent, output),
        SerValue::Mapping(entries) => render_mapping(entries, indent, output),
        SerValue::Tagged(tag, inner) => {
            push_indent(output, indent);
            output.push('!');
            output.push_str(tag);
            if is_inline(inner) {
                output.push(' ');
                render_inline(inner, output);
            } else {
                output.push('\n');
                render_value(inner, indent, output);
            }
        }
    }
}

fn render_sequence(values: &[SerValue], indent: usize, output: &mut String) {
    if values.is_empty() {
        push_indent(output, indent);
        output.push_str("[]");
        return;
    }
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            output.push('\n');
        }
        push_indent(output, indent);
        output.push('-');
        render_nested(value, indent + 2, output);
    }
}

fn render_mapping(entries: &[(SerValue, SerValue)], indent: usize, output: &mut String) {
    if entries.is_empty() {
        push_indent(output, indent);
        output.push_str("{}");
        return;
    }
    for (index, (key, value)) in entries.iter().enumerate() {
        if index > 0 {
            output.push('\n');
        }
        if is_inline(key) {
            push_indent(output, indent);
            render_inline(key, output);
            output.push(':');
        } else {
            push_indent(output, indent);
            output.push('?');
            render_nested(key, indent + 2, output);
            output.push('\n');
            push_indent(output, indent);
            output.push(':');
        }
        render_nested(value, indent + 2, output);
    }
}

fn render_nested(value: &SerValue, indent: usize, output: &mut String) {
    match value {
        SerValue::Tagged(tag, inner) if !is_inline(inner) => {
            output.push(' ');
            output.push('!');
            output.push_str(tag);
            output.push('\n');
            render_value(inner, indent, output);
        }
        _ if is_inline(value) => {
            output.push(' ');
            render_inline(value, output);
        }
        _ => {
            output.push('\n');
            render_value(value, indent, output);
        }
    }
}

fn is_inline(value: &SerValue) -> bool {
    matches!(
        value,
        SerValue::Null
            | SerValue::Bool(_)
            | SerValue::Signed(_)
            | SerValue::Unsigned(_)
            | SerValue::Float(_)
            | SerValue::String(_)
    ) || matches!(value, SerValue::Sequence(values) if values.is_empty())
        || matches!(value, SerValue::Mapping(entries) if entries.is_empty())
        || matches!(value, SerValue::Tagged(_, inner) if is_inline(inner))
}

fn render_inline(value: &SerValue, output: &mut String) {
    match value {
        SerValue::Sequence(values) if values.is_empty() => output.push_str("[]"),
        SerValue::Mapping(entries) if entries.is_empty() => output.push_str("{}"),
        SerValue::Tagged(tag, inner) => {
            output.push('!');
            output.push_str(tag);
            output.push(' ');
            render_inline(inner, output);
        }
        _ => render_scalar(value, output),
    }
}

fn render_scalar(value: &SerValue, output: &mut String) {
    match value {
        SerValue::Null => output.push_str("null"),
        SerValue::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        SerValue::Signed(value) => output.push_str(&value.to_string()),
        SerValue::Unsigned(value) => output.push_str(&value.to_string()),
        SerValue::Float(value) => render_float(*value, output),
        SerValue::String(value) => render_string(value, output),
        _ => unreachable!("collections are not scalars"),
    }
}

fn render_float(value: f64, output: &mut String) {
    if value.is_nan() {
        output.push_str(".nan");
    } else if value == f64::INFINITY {
        output.push_str(".inf");
    } else if value == f64::NEG_INFINITY {
        output.push_str("-.inf");
    } else {
        let text = value.to_string();
        output.push_str(&text);
        if !text.contains(['.', 'e', 'E']) {
            output.push_str(".0");
        }
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
