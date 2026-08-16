use serde::Serialize;
use serde::de::DeserializeOwned;
use serde::ser::{self, SerializeTuple as _};

use super::{Mapping, Number, TAGGED_VALUE_TOKEN, Tag, TaggedValue, Value};
use crate::{Error, Result};

/// Serializer whose output is an in-memory [`Value`].
#[derive(Clone, Copy, Debug, Default)]
pub struct Serializer;

/// Converts a serializable value into a generic YAML [`Value`].
///
/// # Errors
///
/// Returns an error raised by the value's `Serialize` implementation or when
/// it requests a representation unsupported by yaml-rt-serde.
pub fn to_value<T>(value: T) -> Result<Value>
where
    T: Serialize,
{
    value.serialize(Serializer)
}

/// Converts a generic YAML [`Value`] into a typed value.
///
/// # Errors
///
/// Returns an error when the value does not match `T`.
pub fn from_value<T>(value: Value) -> Result<T>
where
    T: DeserializeOwned,
{
    T::deserialize(value)
}

/// Sequence accumulator used by [`Serializer`].
#[doc(hidden)]
pub struct SerializeSequence {
    values: Vec<Value>,
    tag: Option<Tag>,
}

impl SerializeSequence {
    fn new(len: Option<usize>, tag: Option<Tag>) -> Self {
        Self {
            values: Vec::with_capacity(len.unwrap_or(0)),
            tag,
        }
    }

    fn push<T>(&mut self, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        self.values.push(value.serialize(Serializer)?);
        Ok(())
    }

    fn finish(self) -> Value {
        tagged(self.tag, Value::Sequence(self.values))
    }
}

impl ser::SerializeSeq for SerializeSequence {
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

impl ser::SerializeTuple for SerializeSequence {
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

impl ser::SerializeTupleStruct for SerializeSequence {
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

impl ser::SerializeTupleVariant for SerializeSequence {
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

/// Mapping accumulator used by [`Serializer`].
#[doc(hidden)]
pub struct SerializeMapping {
    entries: Mapping,
    pending: Option<Value>,
    tag: Option<Tag>,
}

impl SerializeMapping {
    fn new(len: Option<usize>, tag: Option<Tag>) -> Self {
        Self {
            entries: Mapping::with_capacity(len.unwrap_or(0)),
            pending: None,
            tag,
        }
    }

    fn insert(&mut self, key: Value, value: Value) {
        self.entries.insert(key, value);
    }

    fn finish(self) -> Result<Value> {
        if self.pending.is_some() {
            return Err(Error::message("map ended before serializing a value"));
        }
        Ok(tagged(self.tag, Value::Mapping(self.entries)))
    }
}

impl ser::SerializeMap for SerializeMapping {
    type Ok = Value;
    type Error = Error;

    fn serialize_key<T>(&mut self, key: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        if self.pending.is_some() {
            return Err(Error::message("map key serialized before its value"));
        }
        self.pending = Some(key.serialize(Serializer)?);
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
        self.insert(key, value.serialize(Serializer)?);
        Ok(())
    }

    fn serialize_entry<K, V>(&mut self, key: &K, value: &V) -> Result<()>
    where
        K: ?Sized + Serialize,
        V: ?Sized + Serialize,
    {
        self.insert(key.serialize(Serializer)?, value.serialize(Serializer)?);
        Ok(())
    }

    fn end(self) -> Result<Value> {
        self.finish()
    }
}

impl ser::SerializeStruct for SerializeMapping {
    type Ok = Value;
    type Error = Error;

    fn serialize_field<T>(&mut self, key: &'static str, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        self.insert(Value::String(key.to_owned()), value.serialize(Serializer)?);
        Ok(())
    }

    fn end(self) -> Result<Value> {
        self.finish()
    }
}

impl ser::SerializeStructVariant for SerializeMapping {
    type Ok = Value;
    type Error = Error;

    fn serialize_field<T>(&mut self, key: &'static str, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        ser::SerializeStruct::serialize_field(self, key, value)
    }

    fn end(self) -> Result<Value> {
        self.finish()
    }
}

impl ser::Serializer for Serializer {
    type Ok = Value;
    type Error = Error;
    type SerializeSeq = SerializeSequence;
    type SerializeTuple = SerializeSequence;
    type SerializeTupleStruct = SerializeSequence;
    type SerializeTupleVariant = SerializeSequence;
    type SerializeMap = SerializeMapping;
    type SerializeStruct = SerializeMapping;
    type SerializeStructVariant = SerializeMapping;

    fn serialize_bool(self, value: bool) -> Result<Value> {
        Ok(Value::Bool(value))
    }

    fn serialize_i8(self, value: i8) -> Result<Value> {
        self.serialize_i128(value.into())
    }

    fn serialize_i16(self, value: i16) -> Result<Value> {
        self.serialize_i128(value.into())
    }

    fn serialize_i32(self, value: i32) -> Result<Value> {
        self.serialize_i128(value.into())
    }

    fn serialize_i64(self, value: i64) -> Result<Value> {
        self.serialize_i128(value.into())
    }

    fn serialize_i128(self, value: i128) -> Result<Value> {
        Ok(Value::Number(Number::from(value)))
    }

    fn serialize_u8(self, value: u8) -> Result<Value> {
        self.serialize_u128(value.into())
    }

    fn serialize_u16(self, value: u16) -> Result<Value> {
        self.serialize_u128(value.into())
    }

    fn serialize_u32(self, value: u32) -> Result<Value> {
        self.serialize_u128(value.into())
    }

    fn serialize_u64(self, value: u64) -> Result<Value> {
        self.serialize_u128(value.into())
    }

    fn serialize_u128(self, value: u128) -> Result<Value> {
        Ok(Value::Number(Number::from(value)))
    }

    fn serialize_f32(self, value: f32) -> Result<Value> {
        Ok(Value::Number(Number::from(value)))
    }

    fn serialize_f64(self, value: f64) -> Result<Value> {
        Ok(Value::Number(Number::from(value)))
    }

    fn serialize_char(self, value: char) -> Result<Value> {
        Ok(Value::String(value.to_string()))
    }

    fn serialize_str(self, value: &str) -> Result<Value> {
        Ok(Value::String(value.to_owned()))
    }

    fn serialize_bytes(self, _value: &[u8]) -> Result<Value> {
        Err(Error::message(
            "serialization and deserialization of bytes in YAML is not implemented",
        ))
    }

    fn serialize_none(self) -> Result<Value> {
        Ok(Value::Null)
    }

    fn serialize_some<T>(self, value: &T) -> Result<Value>
    where
        T: ?Sized + Serialize,
    {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<Value> {
        Ok(Value::Null)
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<Value> {
        Ok(Value::Null)
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> Result<Value> {
        Ok(Value::String(variant.to_owned()))
    }

    fn serialize_newtype_struct<T>(self, name: &'static str, value: &T) -> Result<Value>
    where
        T: ?Sized + Serialize,
    {
        let value = value.serialize(self)?;
        if name != TAGGED_VALUE_TOKEN {
            return Ok(value);
        }
        let Value::Sequence(mut parts) = value else {
            return Err(Error::message("invalid tagged value payload"));
        };
        if parts.len() != 2 {
            return Err(Error::message("invalid tagged value payload"));
        }
        let inner = parts.pop().expect("tagged payload contains a value");
        let tag = parts.pop().expect("tagged payload contains a tag");
        let Value::String(tag) = tag else {
            return Err(Error::message("invalid tagged value tag"));
        };
        Ok(Value::Tagged(Box::new(TaggedValue {
            tag: Tag::new(tag),
            value: inner,
        })))
    }

    fn serialize_newtype_variant<T>(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Value>
    where
        T: ?Sized + Serialize,
    {
        let value = value.serialize(self)?;
        if matches!(value, Value::Tagged(_)) {
            return Err(Error::message(
                "serializing nested enums in YAML is not supported",
            ));
        }
        Ok(tagged(Some(Tag::new(variant)), value))
    }

    fn serialize_seq(self, len: Option<usize>) -> Result<SerializeSequence> {
        Ok(SerializeSequence::new(len, None))
    }

    fn serialize_tuple(self, len: usize) -> Result<SerializeSequence> {
        Ok(SerializeSequence::new(Some(len), None))
    }

    fn serialize_tuple_struct(self, _name: &'static str, len: usize) -> Result<SerializeSequence> {
        Ok(SerializeSequence::new(Some(len), None))
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<SerializeSequence> {
        Ok(SerializeSequence::new(Some(len), Some(Tag::new(variant))))
    }

    fn serialize_map(self, len: Option<usize>) -> Result<SerializeMapping> {
        Ok(SerializeMapping::new(len, None))
    }

    fn serialize_struct(self, _name: &'static str, len: usize) -> Result<SerializeMapping> {
        Ok(SerializeMapping::new(Some(len), None))
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<SerializeMapping> {
        Ok(SerializeMapping::new(Some(len), Some(Tag::new(variant))))
    }

    fn collect_str<T>(self, value: &T) -> Result<Value>
    where
        T: ?Sized + std::fmt::Display,
    {
        Ok(Value::String(value.to_string()))
    }

    fn is_human_readable(&self) -> bool {
        true
    }
}

fn tagged(tag: Option<Tag>, value: Value) -> Value {
    match tag {
        Some(tag) => Value::Tagged(Box::new(TaggedValue { tag, value })),
        None => value,
    }
}

struct TaggedPayload<'a>(&'a TaggedValue);

impl Serialize for TaggedPayload<'_> {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut tuple = serializer.serialize_tuple(2)?;
        tuple.serialize_element(&self.0.tag.to_string())?;
        tuple.serialize_element(&self.0.value)?;
        tuple.end()
    }
}

impl Serialize for Value {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Null => serializer.serialize_unit(),
            Self::Bool(value) => serializer.serialize_bool(*value),
            Self::Number(value) => value.serialize(serializer),
            Self::String(value) => serializer.serialize_str(value),
            Self::Sequence(values) => values.serialize(serializer),
            Self::Mapping(mapping) => mapping.serialize(serializer),
            Self::Tagged(tagged) => {
                serializer.serialize_newtype_struct(TAGGED_VALUE_TOKEN, &TaggedPayload(tagged))
            }
        }
    }
}
