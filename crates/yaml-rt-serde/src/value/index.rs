use std::fmt;
use std::ops;

use super::{Mapping, Number, Value};

mod private {
    pub trait Sealed {}

    impl Sealed for usize {}
    impl Sealed for str {}
    impl Sealed for String {}
    impl Sealed for super::Value {}
    impl<T> Sealed for &T where T: ?Sized + Sealed {}
}

/// A sealed type that can index a YAML [`Value`] or [`Mapping`].
pub trait Index: private::Sealed {
    #[doc(hidden)]
    fn index_into<'a>(&self, value: &'a Value) -> Option<&'a Value>;

    #[doc(hidden)]
    fn index_into_mut<'a>(&self, value: &'a mut Value) -> Option<&'a mut Value>;

    #[doc(hidden)]
    fn index_or_insert<'a>(&self, value: &'a mut Value) -> &'a mut Value;

    #[doc(hidden)]
    fn mapping_position(&self, mapping: &Mapping) -> Option<usize>;
}

impl Index for usize {
    fn index_into<'a>(&self, value: &'a Value) -> Option<&'a Value> {
        match value.untag_ref() {
            Value::Sequence(sequence) => sequence.get(*self),
            Value::Mapping(mapping) => mapping.get(Value::Number(Number::from(*self))),
            _ => None,
        }
    }

    fn index_into_mut<'a>(&self, value: &'a mut Value) -> Option<&'a mut Value> {
        match value.untag_mut() {
            Value::Sequence(sequence) => sequence.get_mut(*self),
            Value::Mapping(mapping) => mapping.get_mut(Value::Number(Number::from(*self))),
            _ => None,
        }
    }

    fn index_or_insert<'a>(&self, value: &'a mut Value) -> &'a mut Value {
        match value.untag_mut() {
            Value::Sequence(sequence) => {
                let len = sequence.len();
                sequence.get_mut(*self).unwrap_or_else(|| {
                    panic!("cannot access index {self} of YAML sequence of length {len}")
                })
            }
            Value::Mapping(mapping) => mapping
                .entry(Value::Number(Number::from(*self)))
                .or_insert(Value::Null),
            value => panic!("cannot access index {self} of YAML {}", Type(value)),
        }
    }

    fn mapping_position(&self, mapping: &Mapping) -> Option<usize> {
        mapping.position(&Value::Number(Number::from(*self)))
    }
}

impl Index for Value {
    fn index_into<'a>(&self, value: &'a Value) -> Option<&'a Value> {
        match value.untag_ref() {
            Value::Mapping(mapping) => mapping.get(self),
            _ => None,
        }
    }

    fn index_into_mut<'a>(&self, value: &'a mut Value) -> Option<&'a mut Value> {
        match value.untag_mut() {
            Value::Mapping(mapping) => mapping.get_mut(self),
            _ => None,
        }
    }

    fn index_or_insert<'a>(&self, value: &'a mut Value) -> &'a mut Value {
        if matches!(value, Value::Null) {
            *value = Value::Mapping(Mapping::new());
        }
        match value.untag_mut() {
            Value::Mapping(mapping) => mapping.entry(self.clone()).or_insert(Value::Null),
            value => panic!("cannot access key {self:?} in YAML {}", Type(value)),
        }
    }

    fn mapping_position(&self, mapping: &Mapping) -> Option<usize> {
        mapping.position(self)
    }
}

impl Index for str {
    fn index_into<'a>(&self, value: &'a Value) -> Option<&'a Value> {
        match value.untag_ref() {
            Value::Mapping(mapping) => mapping.get(self),
            _ => None,
        }
    }

    fn index_into_mut<'a>(&self, value: &'a mut Value) -> Option<&'a mut Value> {
        match value.untag_mut() {
            Value::Mapping(mapping) => mapping.get_mut(self),
            _ => None,
        }
    }

    fn index_or_insert<'a>(&self, value: &'a mut Value) -> &'a mut Value {
        if matches!(value, Value::Null) {
            *value = Value::Mapping(Mapping::new());
        }
        match value.untag_mut() {
            Value::Mapping(mapping) => mapping
                .entry(Value::String(self.to_owned()))
                .or_insert(Value::Null),
            value => panic!("cannot access key {self:?} in YAML {}", Type(value)),
        }
    }

    fn mapping_position(&self, mapping: &Mapping) -> Option<usize> {
        mapping
            .entries
            .iter()
            .position(|(key, _)| matches!(key.untag_ref(), Value::String(key) if key == self))
    }
}

impl Index for String {
    fn index_into<'a>(&self, value: &'a Value) -> Option<&'a Value> {
        self.as_str().index_into(value)
    }

    fn index_into_mut<'a>(&self, value: &'a mut Value) -> Option<&'a mut Value> {
        self.as_str().index_into_mut(value)
    }

    fn index_or_insert<'a>(&self, value: &'a mut Value) -> &'a mut Value {
        self.as_str().index_or_insert(value)
    }

    fn mapping_position(&self, mapping: &Mapping) -> Option<usize> {
        self.as_str().mapping_position(mapping)
    }
}

impl<T> Index for &T
where
    T: ?Sized + Index,
{
    fn index_into<'a>(&self, value: &'a Value) -> Option<&'a Value> {
        (**self).index_into(value)
    }

    fn index_into_mut<'a>(&self, value: &'a mut Value) -> Option<&'a mut Value> {
        (**self).index_into_mut(value)
    }

    fn index_or_insert<'a>(&self, value: &'a mut Value) -> &'a mut Value {
        (**self).index_or_insert(value)
    }

    fn mapping_position(&self, mapping: &Mapping) -> Option<usize> {
        (**self).mapping_position(mapping)
    }
}

struct Type<'a>(&'a Value);

impl fmt::Display for Type<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.0 {
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Sequence(_) => "sequence",
            Value::Mapping(_) => "mapping",
            Value::Tagged(_) => "tagged value",
        })
    }
}

impl<I> ops::Index<I> for Value
where
    I: Index,
{
    type Output = Value;

    fn index(&self, index: I) -> &Self::Output {
        static NULL: Value = Value::Null;
        index.index_into(self).unwrap_or(&NULL)
    }
}

impl<I> ops::IndexMut<I> for Value
where
    I: Index,
{
    fn index_mut(&mut self, index: I) -> &mut Self::Output {
        index.index_or_insert(self)
    }
}
