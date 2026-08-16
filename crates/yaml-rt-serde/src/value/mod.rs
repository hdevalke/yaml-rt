//! A loosely typed, presentation-independent YAML value model.

mod de;
mod index;
mod mapping;
mod number;
mod ser;

use std::borrow::Cow;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};

use crate::{Error, Result};

pub use index::Index;
pub use mapping::{
    Entry, IntoIter, IntoKeys, IntoValues, Iter, IterMut, Keys, Mapping, OccupiedEntry,
    VacantEntry, Values, ValuesMut,
};
pub use number::Number;
pub use ser::{Serializer, from_value, to_value};

pub(crate) const TAGGED_VALUE_TOKEN: &str = "$yaml_rt::private::TaggedValue";

/// A YAML sequence whose elements are [`Value`]s.
pub type Sequence = Vec<Value>;

/// A loosely typed YAML value.
#[derive(Clone, Default, PartialEq, PartialOrd, Hash)]
pub enum Value {
    /// YAML null.
    #[default]
    Null,
    /// YAML boolean.
    Bool(bool),
    /// YAML integer or floating-point number.
    Number(Number),
    /// YAML string.
    String(String),
    /// YAML sequence.
    Sequence(Sequence),
    /// YAML mapping.
    Mapping(Mapping),
    /// A locally tagged YAML value.
    Tagged(Box<TaggedValue>),
}

impl Eq for Value {}

impl fmt::Debug for Value {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => formatter.write_str("Null"),
            Self::Bool(value) => formatter.debug_tuple("Bool").field(value).finish(),
            Self::Number(value) => formatter.debug_tuple("Number").field(value).finish(),
            Self::String(value) => formatter.debug_tuple("String").field(value).finish(),
            Self::Sequence(value) => formatter.debug_tuple("Sequence").field(value).finish(),
            Self::Mapping(value) => formatter.debug_tuple("Mapping").field(value).finish(),
            Self::Tagged(value) => formatter.debug_tuple("Tagged").field(value).finish(),
        }
    }
}

impl Value {
    /// Returns the child selected by `index`.
    #[must_use]
    pub fn get<I>(&self, index: I) -> Option<&Value>
    where
        I: Index,
    {
        index.index_into(self)
    }

    /// Returns the mutable child selected by `index`.
    pub fn get_mut<I>(&mut self, index: I) -> Option<&mut Value>
    where
        I: Index,
    {
        index.index_into_mut(self)
    }

    pub(crate) fn untag_ref(&self) -> &Self {
        let mut value = self;
        while let Self::Tagged(tagged) = value {
            value = &tagged.value;
        }
        value
    }

    pub(crate) fn untag_mut(&mut self) -> &mut Self {
        let mut value = self;
        while let Self::Tagged(tagged) = value {
            value = &mut tagged.value;
        }
        value
    }

    /// Returns true for null, including a tagged null.
    #[must_use]
    pub fn is_null(&self) -> bool {
        matches!(self.untag_ref(), Self::Null)
    }

    /// Returns `Some(())` for null.
    #[must_use]
    pub fn as_null(&self) -> Option<()> {
        self.is_null().then_some(())
    }

    /// Returns true for a boolean.
    #[must_use]
    pub fn is_bool(&self) -> bool {
        self.as_bool().is_some()
    }

    /// Returns the boolean value.
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self.untag_ref() {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// Returns true for a number.
    #[must_use]
    pub fn is_number(&self) -> bool {
        matches!(self.untag_ref(), Self::Number(_))
    }

    /// Returns true for an integer representable as `i64`.
    #[must_use]
    pub fn is_i64(&self) -> bool {
        self.as_i64().is_some()
    }

    /// Returns true for an integer representable as `u64`.
    #[must_use]
    pub fn is_u64(&self) -> bool {
        self.as_u64().is_some()
    }

    /// Returns true for a floating-point number.
    #[must_use]
    pub fn is_f64(&self) -> bool {
        matches!(self.untag_ref(), Self::Number(number) if number.is_f64())
    }

    /// Returns true for an integer representable as `i128`.
    #[must_use]
    pub fn is_i128(&self) -> bool {
        self.as_i128().is_some()
    }

    /// Returns true for an integer representable as `u128`.
    #[must_use]
    pub fn is_u128(&self) -> bool {
        self.as_u128().is_some()
    }

    /// Returns the number as `i64` when possible.
    #[must_use]
    pub fn as_i64(&self) -> Option<i64> {
        match self.untag_ref() {
            Self::Number(number) => number.as_i64(),
            _ => None,
        }
    }

    /// Returns the number as `u64` when possible.
    #[must_use]
    pub fn as_u64(&self) -> Option<u64> {
        match self.untag_ref() {
            Self::Number(number) => number.as_u64(),
            _ => None,
        }
    }

    /// Returns the number as `i128` when possible.
    #[must_use]
    pub fn as_i128(&self) -> Option<i128> {
        match self.untag_ref() {
            Self::Number(number) => number.as_i128(),
            _ => None,
        }
    }

    /// Returns the number as `u128` when possible.
    #[must_use]
    pub fn as_u128(&self) -> Option<u128> {
        match self.untag_ref() {
            Self::Number(number) => number.as_u128(),
            _ => None,
        }
    }

    /// Returns the number as `f64` when possible.
    #[must_use]
    pub fn as_f64(&self) -> Option<f64> {
        match self.untag_ref() {
            Self::Number(number) => number.as_f64(),
            _ => None,
        }
    }

    /// Returns true for a string.
    #[must_use]
    pub fn is_string(&self) -> bool {
        self.as_str().is_some()
    }

    /// Returns the string value.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self.untag_ref() {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    /// Returns true for a sequence.
    #[must_use]
    pub fn is_sequence(&self) -> bool {
        self.as_sequence().is_some()
    }

    /// Returns the sequence.
    #[must_use]
    pub fn as_sequence(&self) -> Option<&Sequence> {
        match self.untag_ref() {
            Self::Sequence(value) => Some(value),
            _ => None,
        }
    }

    /// Returns the mutable sequence.
    pub fn as_sequence_mut(&mut self) -> Option<&mut Sequence> {
        match self.untag_mut() {
            Self::Sequence(value) => Some(value),
            _ => None,
        }
    }

    /// Returns true for a mapping.
    #[must_use]
    pub fn is_mapping(&self) -> bool {
        self.as_mapping().is_some()
    }

    /// Returns the mapping.
    #[must_use]
    pub fn as_mapping(&self) -> Option<&Mapping> {
        match self.untag_ref() {
            Self::Mapping(value) => Some(value),
            _ => None,
        }
    }

    /// Returns the mutable mapping.
    pub fn as_mapping_mut(&mut self) -> Option<&mut Mapping> {
        match self.untag_mut() {
            Self::Mapping(value) => Some(value),
            _ => None,
        }
    }

    /// Recursively expands YAML merge (`<<`) entries.
    ///
    /// # Errors
    ///
    /// Returns an error when a merge operand is not a mapping or a sequence of
    /// mappings.
    pub fn apply_merge(&mut self) -> Result<()> {
        match self {
            Self::Sequence(values) => {
                for value in values {
                    value.apply_merge()?;
                }
            }
            Self::Mapping(mapping) => apply_mapping_merge(mapping)?,
            Self::Tagged(tagged) => tagged.value.apply_merge()?,
            Self::Null | Self::Bool(_) | Self::Number(_) | Self::String(_) => {}
        }
        Ok(())
    }
}

fn apply_mapping_merge(mapping: &mut Mapping) -> Result<()> {
    for (_, value) in &mut *mapping {
        value.apply_merge()?;
    }

    let Some(position) = mapping
        .entries
        .iter()
        .position(|(key, _)| matches!(key.untag_ref(), Value::String(key) if key == "<<"))
    else {
        return Ok(());
    };

    let (_, source) = mapping.entries.remove(position);
    let explicit = std::mem::take(mapping);
    let mut merged = Mapping::new();
    merge_source(&mut merged, source.untag_ref())?;
    for (key, value) in explicit {
        merged.insert(key, value);
    }
    *mapping = merged;
    Ok(())
}

fn merge_source(target: &mut Mapping, source: &Value) -> Result<()> {
    match source {
        Value::Mapping(mapping) => {
            merge_mapping(target, mapping);
            Ok(())
        }
        Value::Sequence(sequence) => {
            for value in sequence {
                let Value::Mapping(mapping) = value.untag_ref() else {
                    return Err(Error::message("expected a mapping in YAML merge sequence"));
                };
                merge_mapping(target, mapping);
            }
            Ok(())
        }
        _ => Err(Error::message(
            "expected a mapping or sequence of mappings for YAML merge",
        )),
    }
}

fn merge_mapping(target: &mut Mapping, source: &Mapping) {
    for (key, value) in source {
        if !target.contains_key(key) {
            target.insert(key.clone(), value.clone());
        }
    }
}

/// A normalized YAML local tag.
#[derive(Clone, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct Tag {
    pub(crate) string: String,
}

impl Tag {
    /// Creates a tag. A leading `!` is optional and not significant.
    ///
    /// # Panics
    ///
    /// Panics for an empty tag.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        let value = value.strip_prefix('!').unwrap_or(&value);
        assert!(!value.is_empty(), "YAML tags cannot be empty");
        Self {
            string: value.to_owned(),
        }
    }

    pub(crate) fn as_suffix(&self) -> &str {
        &self.string
    }
}

impl fmt::Display for Tag {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "!{}", self.string)
    }
}

impl fmt::Debug for Tag {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("Tag")
            .field(&self.to_string())
            .finish()
    }
}

impl PartialEq<str> for Tag {
    fn eq(&self, other: &str) -> bool {
        self.string == other.strip_prefix('!').unwrap_or(other)
    }
}

impl PartialEq<&str> for Tag {
    fn eq(&self, other: &&str) -> bool {
        self == *other
    }
}

impl PartialEq<String> for Tag {
    fn eq(&self, other: &String) -> bool {
        self == other.as_str()
    }
}

impl Serialize for Tag {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Tag {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer).map(Self::new)
    }
}

/// A YAML tag and its associated value.
#[derive(Clone, Debug, PartialEq, PartialOrd, Hash)]
pub struct TaggedValue {
    /// The YAML tag.
    pub tag: Tag,
    /// The tagged value.
    pub value: Value,
}

impl Eq for TaggedValue {}

impl Serialize for TaggedValue {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        Value::Tagged(Box::new(self.clone())).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for TaggedValue {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match Value::deserialize(deserializer)? {
            Value::Tagged(value) => Ok(*value),
            _ => Err(serde::de::Error::custom("expected a tagged YAML value")),
        }
    }
}

impl From<Mapping> for Value {
    fn from(value: Mapping) -> Self {
        Self::Mapping(value)
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

impl From<Cow<'_, str>> for Value {
    fn from(value: Cow<'_, str>) -> Self {
        Self::String(value.into_owned())
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

macro_rules! number_conversions {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl From<$ty> for Value {
                fn from(value: $ty) -> Self {
                    Self::Number(Number::from(value))
                }
            }

            impl PartialEq<$ty> for Value {
                fn eq(&self, other: &$ty) -> bool {
                    self == &Self::from(*other)
                }
            }

            impl PartialEq<Value> for $ty {
                fn eq(&self, other: &Value) -> bool {
                    &Value::from(*self) == other
                }
            }
        )+
    };
}

number_conversions!(
    i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize, f32, f64,
);

impl<T> From<Vec<T>> for Value
where
    T: Into<Value>,
{
    fn from(values: Vec<T>) -> Self {
        Self::Sequence(values.into_iter().map(Into::into).collect())
    }
}

impl<T> From<&[T]> for Value
where
    T: Clone + Into<Value>,
{
    fn from(values: &[T]) -> Self {
        Self::Sequence(values.iter().cloned().map(Into::into).collect())
    }
}

impl<T> FromIterator<T> for Value
where
    T: Into<Value>,
{
    fn from_iter<I>(iter: I) -> Self
    where
        I: IntoIterator<Item = T>,
    {
        Self::Sequence(iter.into_iter().map(Into::into).collect())
    }
}

impl PartialEq<str> for Value {
    fn eq(&self, other: &str) -> bool {
        self.as_str().is_some_and(|value| value == other)
    }
}

impl PartialEq<&str> for Value {
    fn eq(&self, other: &&str) -> bool {
        self == *other
    }
}

impl PartialEq<String> for Value {
    fn eq(&self, other: &String) -> bool {
        self == other.as_str()
    }
}

impl PartialEq<bool> for Value {
    fn eq(&self, other: &bool) -> bool {
        self.as_bool().is_some_and(|value| value == *other)
    }
}

impl PartialEq<Value> for bool {
    fn eq(&self, other: &Value) -> bool {
        other == self
    }
}

impl PartialEq<Value> for str {
    fn eq(&self, other: &Value) -> bool {
        other == self
    }
}

impl PartialEq<Value> for String {
    fn eq(&self, other: &Value) -> bool {
        other == self
    }
}
