use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::str::FromStr;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{Error, Result};

/// A YAML integer or floating-point number.
///
/// Unlike `yaml_serde::Number`, this type retains the full Serde `i128` and
/// `u128` ranges.
#[derive(Clone, Copy)]
pub struct Number {
    repr: Repr,
}

#[derive(Clone, Copy)]
enum Repr {
    Signed(i128),
    Unsigned(u128),
    Float(f64),
}

impl Number {
    pub(crate) const fn signed(value: i128) -> Self {
        Self {
            repr: Repr::Signed(value),
        }
    }

    pub(crate) const fn unsigned(value: u128) -> Self {
        Self {
            repr: Repr::Unsigned(value),
        }
    }

    pub(crate) const fn float(value: f64) -> Self {
        Self {
            repr: Repr::Float(value),
        }
    }

    /// Returns true when this is an integer representable as `i64`.
    #[must_use]
    pub fn is_i64(&self) -> bool {
        self.as_i64().is_some()
    }

    /// Returns true when this is an integer representable as `u64`.
    #[must_use]
    pub fn is_u64(&self) -> bool {
        self.as_u64().is_some()
    }

    /// Returns true when this was represented as a floating-point number.
    #[must_use]
    pub const fn is_f64(&self) -> bool {
        matches!(self.repr, Repr::Float(_))
    }

    /// Returns true when this is an integer representable as `i128`.
    #[must_use]
    pub fn is_i128(&self) -> bool {
        self.as_i128().is_some()
    }

    /// Returns true when this is an integer representable as `u128`.
    #[must_use]
    pub fn is_u128(&self) -> bool {
        self.as_u128().is_some()
    }

    /// Returns the integer as `i64` when it fits.
    #[must_use]
    pub fn as_i64(&self) -> Option<i64> {
        self.as_i128().and_then(|value| i64::try_from(value).ok())
    }

    /// Returns the integer as `u64` when it fits.
    #[must_use]
    pub fn as_u64(&self) -> Option<u64> {
        self.as_u128().and_then(|value| u64::try_from(value).ok())
    }

    /// Returns the integer as `i128` when it fits.
    #[must_use]
    pub fn as_i128(&self) -> Option<i128> {
        match self.repr {
            Repr::Signed(value) => Some(value),
            Repr::Unsigned(value) => i128::try_from(value).ok(),
            Repr::Float(_) => None,
        }
    }

    /// Returns the integer as `u128` when it is non-negative.
    #[must_use]
    pub fn as_u128(&self) -> Option<u128> {
        match self.repr {
            Repr::Signed(value) => u128::try_from(value).ok(),
            Repr::Unsigned(value) => Some(value),
            Repr::Float(_) => None,
        }
    }

    /// Returns this number as `f64`.
    #[must_use]
    pub fn as_f64(&self) -> Option<f64> {
        Some(match self.repr {
            Repr::Signed(value) => value as f64,
            Repr::Unsigned(value) => value as f64,
            Repr::Float(value) => value,
        })
    }

    /// Returns true when this is NaN.
    #[must_use]
    pub fn is_nan(&self) -> bool {
        matches!(self.repr, Repr::Float(value) if value.is_nan())
    }

    /// Returns true when this is positive or negative infinity.
    #[must_use]
    pub fn is_infinite(&self) -> bool {
        matches!(self.repr, Repr::Float(value) if value.is_infinite())
    }

    /// Returns true when this is not infinity or NaN.
    #[must_use]
    pub fn is_finite(&self) -> bool {
        !matches!(self.repr, Repr::Float(value) if !value.is_finite())
    }
}

impl PartialEq for Number {
    fn eq(&self, other: &Self) -> bool {
        match (self.repr, other.repr) {
            (Repr::Signed(left), Repr::Signed(right)) => left == right,
            (Repr::Unsigned(left), Repr::Unsigned(right)) => left == right,
            (Repr::Signed(left), Repr::Unsigned(right))
            | (Repr::Unsigned(right), Repr::Signed(left)) => {
                u128::try_from(left).is_ok_and(|left| left == right)
            }
            (Repr::Float(left), Repr::Float(right)) => left == right,
            _ => false,
        }
    }
}

impl PartialOrd for Number {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        match (self.repr, other.repr) {
            (Repr::Signed(left), Repr::Signed(right)) => left.partial_cmp(&right),
            (Repr::Unsigned(left), Repr::Unsigned(right)) => left.partial_cmp(&right),
            (Repr::Signed(left), Repr::Unsigned(right)) => {
                if left < 0 {
                    Some(Ordering::Less)
                } else {
                    (left as u128).partial_cmp(&right)
                }
            }
            (Repr::Unsigned(left), Repr::Signed(right)) => {
                if right < 0 {
                    Some(Ordering::Greater)
                } else {
                    left.partial_cmp(&(right as u128))
                }
            }
            (Repr::Float(left), Repr::Float(right)) => left.partial_cmp(&right),
            (left, right) => repr_as_f64(left).partial_cmp(&repr_as_f64(right)),
        }
    }
}

fn repr_as_f64(value: Repr) -> f64 {
    match value {
        Repr::Signed(value) => value as f64,
        Repr::Unsigned(value) => value as f64,
        Repr::Float(value) => value,
    }
}

impl Hash for Number {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self.repr {
            Repr::Signed(value) if value >= 0 => {
                0_u8.hash(state);
                (value as u128).hash(state);
            }
            Repr::Signed(value) => {
                1_u8.hash(state);
                value.hash(state);
            }
            Repr::Unsigned(value) => {
                0_u8.hash(state);
                value.hash(state);
            }
            Repr::Float(value) => {
                2_u8.hash(state);
                let bits = if value == 0.0 { 0 } else { value.to_bits() };
                bits.hash(state);
            }
        }
    }
}

impl fmt::Display for Number {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.repr {
            Repr::Signed(value) => value.fmt(formatter),
            Repr::Unsigned(value) => value.fmt(formatter),
            Repr::Float(value) if value.is_nan() => formatter.write_str(".nan"),
            Repr::Float(value) if value == f64::INFINITY => formatter.write_str(".inf"),
            Repr::Float(value) if value == f64::NEG_INFINITY => formatter.write_str("-.inf"),
            Repr::Float(value) => {
                let text = value.to_string();
                formatter.write_str(&text)?;
                if !text.contains(['.', 'e', 'E']) {
                    formatter.write_str(".0")?;
                }
                Ok(())
            }
        }
    }
}

impl fmt::Debug for Number {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl FromStr for Number {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        match crate::from_str::<super::Value>(value)? {
            super::Value::Number(number) => Ok(number),
            _ => Err(Error::message("expected a YAML number")),
        }
    }
}

macro_rules! from_signed {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl From<$ty> for Number {
                fn from(value: $ty) -> Self {
                    Self::signed(value as i128)
                }
            }
        )+
    };
}

macro_rules! from_unsigned {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl From<$ty> for Number {
                fn from(value: $ty) -> Self {
                    Self::unsigned(value as u128)
                }
            }
        )+
    };
}

from_signed!(i8, i16, i32, i64, i128, isize);
from_unsigned!(u8, u16, u32, u64, u128, usize);

impl From<f32> for Number {
    fn from(value: f32) -> Self {
        Self::float(value.into())
    }
}

impl From<f64> for Number {
    fn from(value: f64) -> Self {
        Self::float(value)
    }
}

impl Serialize for Number {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self.repr {
            Repr::Signed(value) => serializer.serialize_i128(value),
            Repr::Unsigned(value) => serializer.serialize_u128(value),
            Repr::Float(value) => serializer.serialize_f64(value),
        }
    }
}

impl<'de> Deserialize<'de> for Number {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct NumberVisitor;

        impl<'de> Visitor<'de> for NumberVisitor {
            type Value = Number;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a YAML number")
            }

            fn visit_i64<E>(self, value: i64) -> std::result::Result<Number, E> {
                Ok(Number::from(value))
            }

            fn visit_i128<E>(self, value: i128) -> std::result::Result<Number, E> {
                Ok(Number::from(value))
            }

            fn visit_u64<E>(self, value: u64) -> std::result::Result<Number, E> {
                Ok(Number::from(value))
            }

            fn visit_u128<E>(self, value: u128) -> std::result::Result<Number, E> {
                Ok(Number::from(value))
            }

            fn visit_f64<E>(self, value: f64) -> std::result::Result<Number, E> {
                Ok(Number::from(value))
            }
        }

        deserializer.deserialize_any(NumberVisitor)
    }
}

impl<'de> de::IntoDeserializer<'de, Error> for Number {
    type Deserializer = super::Value;

    fn into_deserializer(self) -> Self::Deserializer {
        super::Value::Number(self)
    }
}
