use std::fmt;

use serde::de::{
    self, DeserializeSeed, EnumAccess, IntoDeserializer, MapAccess, SeqAccess, VariantAccess,
    Visitor,
};
use serde::{Deserialize, Deserializer};

use super::{Mapping, Number, Tag, TaggedValue, Value};
use crate::{Error, Result};

impl<'de> Deserialize<'de> for Value {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(ValueVisitor)
    }
}

struct ValueVisitor;

impl<'de> Visitor<'de> for ValueVisitor {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("any YAML value")
    }

    fn visit_unit<E>(self) -> std::result::Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_none<E>(self) -> std::result::Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_some<D>(self, deserializer: D) -> std::result::Result<Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        Value::deserialize(deserializer)
    }

    fn visit_bool<E>(self, value: bool) -> std::result::Result<Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_i8<E>(self, value: i8) -> std::result::Result<Value, E> {
        Ok(Value::from(value))
    }

    fn visit_i16<E>(self, value: i16) -> std::result::Result<Value, E> {
        Ok(Value::from(value))
    }

    fn visit_i32<E>(self, value: i32) -> std::result::Result<Value, E> {
        Ok(Value::from(value))
    }

    fn visit_i64<E>(self, value: i64) -> std::result::Result<Value, E> {
        Ok(Value::from(value))
    }

    fn visit_i128<E>(self, value: i128) -> std::result::Result<Value, E> {
        Ok(Value::from(value))
    }

    fn visit_u8<E>(self, value: u8) -> std::result::Result<Value, E> {
        Ok(Value::from(value))
    }

    fn visit_u16<E>(self, value: u16) -> std::result::Result<Value, E> {
        Ok(Value::from(value))
    }

    fn visit_u32<E>(self, value: u32) -> std::result::Result<Value, E> {
        Ok(Value::from(value))
    }

    fn visit_u64<E>(self, value: u64) -> std::result::Result<Value, E> {
        Ok(Value::from(value))
    }

    fn visit_u128<E>(self, value: u128) -> std::result::Result<Value, E> {
        Ok(Value::from(value))
    }

    fn visit_f32<E>(self, value: f32) -> std::result::Result<Value, E> {
        Ok(Value::from(value))
    }

    fn visit_f64<E>(self, value: f64) -> std::result::Result<Value, E> {
        Ok(Value::from(value))
    }

    fn visit_char<E>(self, value: char) -> std::result::Result<Value, E> {
        Ok(Value::String(value.to_string()))
    }

    fn visit_str<E>(self, value: &str) -> std::result::Result<Value, E> {
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E>(self, value: String) -> std::result::Result<Value, E> {
        Ok(Value::String(value))
    }

    fn visit_seq<A>(self, mut access: A) -> std::result::Result<Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::with_capacity(access.size_hint().unwrap_or(0));
        while let Some(value) = access.next_element()? {
            values.push(value);
        }
        Ok(Value::Sequence(values))
    }

    fn visit_map<A>(self, mut access: A) -> std::result::Result<Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut mapping = Mapping::with_capacity(access.size_hint().unwrap_or(0));
        while let Some((key, value)) = access.next_entry()? {
            if mapping.insert(key, value).is_some() {
                return Err(de::Error::custom("duplicate mapping key"));
            }
        }
        Ok(Value::Mapping(mapping))
    }

    fn visit_enum<A>(self, access: A) -> std::result::Result<Value, A::Error>
    where
        A: EnumAccess<'de>,
    {
        let (tag, variant) = access.variant_seed(StringSeed)?;
        let value = variant.newtype_variant::<Value>()?;
        Ok(Value::Tagged(Box::new(TaggedValue {
            tag: Tag::new(tag),
            value,
        })))
    }
}

struct StringSeed;

impl<'de> DeserializeSeed<'de> for StringSeed {
    type Value = String;

    fn deserialize<D>(self, deserializer: D) -> std::result::Result<String, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)
    }
}

macro_rules! deserialize_numeric_methods {
    () => {
        fn deserialize_i8<V>(self, visitor: V) -> Result<V::Value>
        where
            V: Visitor<'de>,
        {
            deserialize_signed(
                self,
                visitor,
                |value| i8::try_from(value).ok(),
                Visitor::visit_i8,
            )
        }

        fn deserialize_i16<V>(self, visitor: V) -> Result<V::Value>
        where
            V: Visitor<'de>,
        {
            deserialize_signed(
                self,
                visitor,
                |value| i16::try_from(value).ok(),
                Visitor::visit_i16,
            )
        }

        fn deserialize_i32<V>(self, visitor: V) -> Result<V::Value>
        where
            V: Visitor<'de>,
        {
            deserialize_signed(
                self,
                visitor,
                |value| i32::try_from(value).ok(),
                Visitor::visit_i32,
            )
        }

        fn deserialize_i64<V>(self, visitor: V) -> Result<V::Value>
        where
            V: Visitor<'de>,
        {
            deserialize_signed(
                self,
                visitor,
                |value| i64::try_from(value).ok(),
                Visitor::visit_i64,
            )
        }

        fn deserialize_i128<V>(self, visitor: V) -> Result<V::Value>
        where
            V: Visitor<'de>,
        {
            deserialize_signed(self, visitor, Some, Visitor::visit_i128)
        }

        fn deserialize_u8<V>(self, visitor: V) -> Result<V::Value>
        where
            V: Visitor<'de>,
        {
            deserialize_unsigned(
                self,
                visitor,
                |value| u8::try_from(value).ok(),
                Visitor::visit_u8,
            )
        }

        fn deserialize_u16<V>(self, visitor: V) -> Result<V::Value>
        where
            V: Visitor<'de>,
        {
            deserialize_unsigned(
                self,
                visitor,
                |value| u16::try_from(value).ok(),
                Visitor::visit_u16,
            )
        }

        fn deserialize_u32<V>(self, visitor: V) -> Result<V::Value>
        where
            V: Visitor<'de>,
        {
            deserialize_unsigned(
                self,
                visitor,
                |value| u32::try_from(value).ok(),
                Visitor::visit_u32,
            )
        }

        fn deserialize_u64<V>(self, visitor: V) -> Result<V::Value>
        where
            V: Visitor<'de>,
        {
            deserialize_unsigned(
                self,
                visitor,
                |value| u64::try_from(value).ok(),
                Visitor::visit_u64,
            )
        }

        fn deserialize_u128<V>(self, visitor: V) -> Result<V::Value>
        where
            V: Visitor<'de>,
        {
            deserialize_unsigned(self, visitor, Some, Visitor::visit_u128)
        }

        fn deserialize_f32<V>(self, visitor: V) -> Result<V::Value>
        where
            V: Visitor<'de>,
        {
            deserialize_float(self, visitor, |visitor, value| {
                let value = checked_f64_to_f32(value)
                    .ok_or_else(|| Error::message("expected an f32 in range"))?;
                visitor.visit_f32(value)
            })
        }

        fn deserialize_f64<V>(self, visitor: V) -> Result<V::Value>
        where
            V: Visitor<'de>,
        {
            deserialize_float(self, visitor, Visitor::visit_f64)
        }
    };
}

impl<'de> de::Deserializer<'de> for Value {
    type Error = Error;

    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        match self {
            Self::Null => visitor.visit_unit(),
            Self::Bool(value) => visitor.visit_bool(value),
            Self::Number(number) => visit_number(number, visitor),
            Self::String(value) => visitor.visit_string(value),
            Self::Sequence(values) => visitor.visit_seq(OwnedSeqAccess {
                values: values.into_iter(),
            }),
            Self::Mapping(mapping) => visitor.visit_map(OwnedMapAccess {
                entries: mapping.into_iter(),
                pending: None,
            }),
            Self::Tagged(tagged) => visitor.visit_enum(OwnedTaggedAccess { tagged: *tagged }),
        }
    }

    fn deserialize_option<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        if self.is_null() {
            visitor.visit_none()
        } else {
            visitor.visit_some(self)
        }
    }

    fn deserialize_enum<V>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        match self {
            Self::Tagged(tagged) => visitor.visit_enum(OwnedTaggedAccess { tagged: *tagged }),
            Self::String(value) => visitor.visit_enum(value.into_deserializer()),
            _ => Err(Error::message("expected a YAML enum")),
        }
    }

    fn deserialize_newtype_struct<V>(self, _name: &'static str, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_ignored_any<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        visitor.visit_unit()
    }

    deserialize_numeric_methods!();

    serde::forward_to_deserialize_any! {
        bool char str string
        bytes byte_buf unit unit_struct seq tuple tuple_struct map struct identifier
    }

    fn is_human_readable(&self) -> bool {
        true
    }
}

impl<'de> de::Deserializer<'de> for &'de Value {
    type Error = Error;

    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        match self {
            Value::Null => visitor.visit_unit(),
            Value::Bool(value) => visitor.visit_bool(*value),
            Value::Number(number) => visit_number(*number, visitor),
            Value::String(value) => visitor.visit_borrowed_str(value),
            Value::Sequence(values) => visitor.visit_seq(BorrowedSeqAccess {
                values: values.iter(),
            }),
            Value::Mapping(mapping) => visitor.visit_map(BorrowedMapAccess {
                entries: mapping.entries.iter(),
                pending: None,
            }),
            Value::Tagged(tagged) => visitor.visit_enum(BorrowedTaggedAccess { tagged }),
        }
    }

    fn deserialize_option<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        if self.is_null() {
            visitor.visit_none()
        } else {
            visitor.visit_some(self)
        }
    }

    fn deserialize_enum<V>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        match self {
            Value::Tagged(tagged) => visitor.visit_enum(BorrowedTaggedAccess { tagged }),
            Value::String(value) => visitor.visit_enum(
                serde::de::value::BorrowedStrDeserializer::<Error>::new(value.as_str()),
            ),
            _ => Err(Error::message("expected a YAML enum")),
        }
    }

    fn deserialize_newtype_struct<V>(self, _name: &'static str, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_ignored_any<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        visitor.visit_unit()
    }

    deserialize_numeric_methods!();

    serde::forward_to_deserialize_any! {
        bool char str string
        bytes byte_buf unit unit_struct seq tuple tuple_struct map struct identifier
    }

    fn is_human_readable(&self) -> bool {
        true
    }
}

fn visit_number<'de, V>(number: Number, visitor: V) -> Result<V::Value>
where
    V: Visitor<'de>,
{
    if number.is_f64() {
        visitor.visit_f64(number.as_f64().expect("float is representable"))
    } else if let Some(value) = number.as_i128()
        && value < 0
    {
        visitor.visit_i128(value)
    } else if let Some(value) = number.as_u128() {
        visitor.visit_u128(value)
    } else if let Some(value) = number.as_i128() {
        visitor.visit_i128(value)
    } else {
        Err(Error::message("invalid YAML number"))
    }
}

trait NumericValue {
    fn into_number(self) -> Result<Number>;
}

impl NumericValue for Value {
    fn into_number(self) -> Result<Number> {
        match self {
            Self::Number(number) => Ok(number),
            _ => Err(Error::message("expected a number")),
        }
    }
}

impl NumericValue for &Value {
    fn into_number(self) -> Result<Number> {
        match self {
            Value::Number(number) => Ok(*number),
            _ => Err(Error::message("expected a number")),
        }
    }
}

fn deserialize_signed<'de, V, T>(
    value: impl NumericValue,
    visitor: V,
    convert: impl FnOnce(i128) -> Option<T>,
    visit: impl FnOnce(V, T) -> Result<V::Value>,
) -> Result<V::Value>
where
    V: Visitor<'de>,
{
    let value = value
        .into_number()?
        .as_i128()
        .and_then(convert)
        .ok_or_else(|| Error::message("expected an integer in range"))?;
    visit(visitor, value)
}

fn deserialize_unsigned<'de, V, T>(
    value: impl NumericValue,
    visitor: V,
    convert: impl FnOnce(u128) -> Option<T>,
    visit: impl FnOnce(V, T) -> Result<V::Value>,
) -> Result<V::Value>
where
    V: Visitor<'de>,
{
    let value = value
        .into_number()?
        .as_u128()
        .and_then(convert)
        .ok_or_else(|| Error::message("expected an unsigned integer in range"))?;
    visit(visitor, value)
}

fn deserialize_float<'de, V>(
    value: impl NumericValue,
    visitor: V,
    visit: impl FnOnce(V, f64) -> Result<V::Value>,
) -> Result<V::Value>
where
    V: Visitor<'de>,
{
    let value = value
        .into_number()?
        .as_f64()
        .ok_or_else(|| Error::message("expected a number"))?;
    visit(visitor, value)
}

fn checked_f64_to_f32(value: f64) -> Option<f32> {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "f32 deserialization applies Rust narrowing and then rejects finite overflow"
    )]
    let converted = value as f32;
    (!value.is_finite() || converted.is_finite()).then_some(converted)
}

struct OwnedSeqAccess {
    values: std::vec::IntoIter<Value>,
}

impl<'de> SeqAccess<'de> for OwnedSeqAccess {
    type Error = Error;

    fn next_element_seed<T>(&mut self, seed: T) -> Result<Option<T::Value>>
    where
        T: DeserializeSeed<'de>,
    {
        self.values
            .next()
            .map(|value| seed.deserialize(value))
            .transpose()
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.values.len())
    }
}

struct BorrowedSeqAccess<'a> {
    values: std::slice::Iter<'a, Value>,
}

impl<'de> SeqAccess<'de> for BorrowedSeqAccess<'de> {
    type Error = Error;

    fn next_element_seed<T>(&mut self, seed: T) -> Result<Option<T::Value>>
    where
        T: DeserializeSeed<'de>,
    {
        self.values
            .next()
            .map(|value| seed.deserialize(value))
            .transpose()
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.values.len())
    }
}

struct OwnedMapAccess {
    entries: super::IntoIter,
    pending: Option<Value>,
}

impl<'de> MapAccess<'de> for OwnedMapAccess {
    type Error = Error;

    fn next_key_seed<K>(&mut self, seed: K) -> Result<Option<K::Value>>
    where
        K: DeserializeSeed<'de>,
    {
        let Some((key, value)) = self.entries.next() else {
            return Ok(None);
        };
        self.pending = Some(value);
        seed.deserialize(key).map(Some)
    }

    fn next_value_seed<V>(&mut self, seed: V) -> Result<V::Value>
    where
        V: DeserializeSeed<'de>,
    {
        seed.deserialize(
            self.pending
                .take()
                .ok_or_else(|| Error::message("value requested without a key"))?,
        )
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.entries.len())
    }
}

struct BorrowedMapAccess<'a> {
    entries: std::slice::Iter<'a, (Value, Value)>,
    pending: Option<&'a Value>,
}

impl<'de> MapAccess<'de> for BorrowedMapAccess<'de> {
    type Error = Error;

    fn next_key_seed<K>(&mut self, seed: K) -> Result<Option<K::Value>>
    where
        K: DeserializeSeed<'de>,
    {
        let Some((key, value)) = self.entries.next() else {
            return Ok(None);
        };
        self.pending = Some(value);
        seed.deserialize(key).map(Some)
    }

    fn next_value_seed<V>(&mut self, seed: V) -> Result<V::Value>
    where
        V: DeserializeSeed<'de>,
    {
        seed.deserialize(
            self.pending
                .take()
                .ok_or_else(|| Error::message("value requested without a key"))?,
        )
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.entries.len())
    }
}

struct OwnedTaggedAccess {
    tagged: TaggedValue,
}

impl<'de> EnumAccess<'de> for OwnedTaggedAccess {
    type Error = Error;
    type Variant = OwnedTaggedVariant;

    fn variant_seed<V>(self, seed: V) -> Result<(V::Value, Self::Variant)>
    where
        V: DeserializeSeed<'de>,
    {
        let variant = seed.deserialize(serde::de::value::StringDeserializer::<Error>::new(
            self.tagged.tag.string,
        ))?;
        Ok((
            variant,
            OwnedTaggedVariant {
                value: self.tagged.value,
            },
        ))
    }
}

struct OwnedTaggedVariant {
    value: Value,
}

impl<'de> VariantAccess<'de> for OwnedTaggedVariant {
    type Error = Error;

    fn unit_variant(self) -> Result<()> {
        if self.value.is_null() {
            Ok(())
        } else {
            Err(Error::message("expected a unit variant"))
        }
    }

    fn newtype_variant_seed<T>(self, seed: T) -> Result<T::Value>
    where
        T: DeserializeSeed<'de>,
    {
        seed.deserialize(self.value)
    }

    fn tuple_variant<V>(self, _len: usize, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        de::Deserializer::deserialize_seq(self.value, visitor)
    }

    fn struct_variant<V>(self, _fields: &'static [&'static str], visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        de::Deserializer::deserialize_map(self.value, visitor)
    }
}

struct BorrowedTaggedAccess<'a> {
    tagged: &'a TaggedValue,
}

impl<'de> EnumAccess<'de> for BorrowedTaggedAccess<'de> {
    type Error = Error;
    type Variant = BorrowedTaggedVariant<'de>;

    fn variant_seed<V>(self, seed: V) -> Result<(V::Value, Self::Variant)>
    where
        V: DeserializeSeed<'de>,
    {
        let variant = seed.deserialize(serde::de::value::BorrowedStrDeserializer::<Error>::new(
            self.tagged.tag.string.as_str(),
        ))?;
        Ok((
            variant,
            BorrowedTaggedVariant {
                value: &self.tagged.value,
            },
        ))
    }
}

struct BorrowedTaggedVariant<'a> {
    value: &'a Value,
}

impl<'de> VariantAccess<'de> for BorrowedTaggedVariant<'de> {
    type Error = Error;

    fn unit_variant(self) -> Result<()> {
        if self.value.is_null() {
            Ok(())
        } else {
            Err(Error::message("expected a unit variant"))
        }
    }

    fn newtype_variant_seed<T>(self, seed: T) -> Result<T::Value>
    where
        T: DeserializeSeed<'de>,
    {
        seed.deserialize(self.value)
    }

    fn tuple_variant<V>(self, _len: usize, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        de::Deserializer::deserialize_seq(self.value, visitor)
    }

    fn struct_variant<V>(self, _fields: &'static [&'static str], visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        de::Deserializer::deserialize_map(self.value, visitor)
    }
}

impl<'de> de::IntoDeserializer<'de, Error> for Value {
    type Deserializer = Self;

    fn into_deserializer(self) -> Self::Deserializer {
        self
    }
}

impl<'de> de::IntoDeserializer<'de, Error> for &'de Value {
    type Deserializer = Self;

    fn into_deserializer(self) -> Self::Deserializer {
        self
    }
}
