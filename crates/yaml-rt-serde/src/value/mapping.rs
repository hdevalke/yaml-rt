use std::cmp::Ordering;
use std::collections::hash_map::DefaultHasher;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::iter::FusedIterator;
use std::ops;

use serde::de::{Error as _, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{Index, Value};

/// An insertion-ordered YAML mapping whose keys and values are both [`Value`].
#[derive(Clone, Default)]
pub struct Mapping {
    pub(crate) entries: Vec<(Value, Value)>,
}

impl Mapping {
    /// Creates an empty mapping.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Creates an empty mapping with space for at least `capacity` entries.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: Vec::with_capacity(capacity),
        }
    }

    /// Reserves capacity for at least `additional` more entries.
    pub fn reserve(&mut self, additional: usize) {
        self.entries.reserve(additional);
    }

    /// Shrinks the mapping's allocation as much as possible.
    pub fn shrink_to_fit(&mut self) {
        self.entries.shrink_to_fit();
    }

    /// Returns the allocated entry capacity.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.entries.capacity()
    }

    /// Returns the number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns true when there are no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Removes all entries.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Inserts an entry, returning the previous value when the key existed.
    pub fn insert(&mut self, key: Value, value: Value) -> Option<Value> {
        if let Some(index) = self.position(&key) {
            return Some(std::mem::replace(&mut self.entries[index].1, value));
        }
        self.entries.push((key, value));
        None
    }

    pub(crate) fn position(&self, key: &Value) -> Option<usize> {
        self.entries
            .iter()
            .position(|(candidate, _)| candidate == key)
    }

    /// Returns whether the mapping contains `index`.
    #[must_use]
    pub fn contains_key<I>(&self, index: I) -> bool
    where
        I: Index,
    {
        index.mapping_position(self).is_some()
    }

    /// Returns the value selected by `index`.
    #[must_use]
    pub fn get<I>(&self, index: I) -> Option<&Value>
    where
        I: Index,
    {
        index
            .mapping_position(self)
            .map(|position| &self.entries[position].1)
    }

    /// Returns a mutable value selected by `index`.
    pub fn get_mut<I>(&mut self, index: I) -> Option<&mut Value>
    where
        I: Index,
    {
        let position = index.mapping_position(self)?;
        Some(&mut self.entries[position].1)
    }

    /// Returns the entry API for `key`.
    pub fn entry(&mut self, key: Value) -> Entry<'_> {
        match self.position(&key) {
            Some(index) => Entry::Occupied(OccupiedEntry {
                mapping: self,
                index,
            }),
            None => Entry::Vacant(VacantEntry { mapping: self, key }),
        }
    }

    /// Removes an entry by swapping the last entry into its position.
    pub fn remove<I>(&mut self, index: I) -> Option<Value>
    where
        I: Index,
    {
        self.swap_remove(index)
    }

    /// Removes and returns an entry by swapping the last entry into its position.
    pub fn remove_entry<I>(&mut self, index: I) -> Option<(Value, Value)>
    where
        I: Index,
    {
        self.swap_remove_entry(index)
    }

    /// Removes a value by swapping the last entry into its position.
    pub fn swap_remove<I>(&mut self, index: I) -> Option<Value>
    where
        I: Index,
    {
        self.swap_remove_entry(index).map(|(_, value)| value)
    }

    /// Removes an entry by swapping the last entry into its position.
    pub fn swap_remove_entry<I>(&mut self, index: I) -> Option<(Value, Value)>
    where
        I: Index,
    {
        let position = index.mapping_position(self)?;
        Some(self.entries.swap_remove(position))
    }

    /// Removes a value while retaining the relative order of other entries.
    pub fn shift_remove<I>(&mut self, index: I) -> Option<Value>
    where
        I: Index,
    {
        self.shift_remove_entry(index).map(|(_, value)| value)
    }

    /// Removes an entry while retaining the relative order of other entries.
    pub fn shift_remove_entry<I>(&mut self, index: I) -> Option<(Value, Value)>
    where
        I: Index,
    {
        let position = index.mapping_position(self)?;
        Some(self.entries.remove(position))
    }

    /// Retains only entries for which `keep` returns true.
    pub fn retain<F>(&mut self, mut keep: F)
    where
        F: FnMut(&Value, &mut Value) -> bool,
    {
        self.entries.retain_mut(|(key, value)| keep(key, value));
    }

    /// Iterates over entries in insertion order.
    pub fn iter(&self) -> Iter<'_> {
        Iter(self.entries.iter())
    }

    /// Mutably iterates over entries in insertion order.
    pub fn iter_mut(&mut self) -> IterMut<'_> {
        IterMut(self.entries.iter_mut())
    }

    /// Iterates over keys in insertion order.
    pub fn keys(&self) -> Keys<'_> {
        Keys(self.iter())
    }

    /// Iterates over values in insertion order.
    pub fn values(&self) -> Values<'_> {
        Values(self.iter())
    }

    /// Mutably iterates over values in insertion order.
    pub fn values_mut(&mut self) -> ValuesMut<'_> {
        ValuesMut(self.iter_mut())
    }

    /// Consumes the mapping and iterates over its keys.
    pub fn into_keys(self) -> IntoKeys {
        IntoKeys(self.entries.into_iter())
    }

    /// Consumes the mapping and iterates over its values.
    pub fn into_values(self) -> IntoValues {
        IntoValues(self.entries.into_iter())
    }
}

impl fmt::Debug for Mapping {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_map().entries(self.iter()).finish()
    }
}

impl PartialEq for Mapping {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len()
            && self
                .iter()
                .all(|(key, value)| other.get(key).is_some_and(|other| other == value))
    }
}

impl Eq for Mapping {}

impl PartialOrd for Mapping {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        if self == other {
            Some(Ordering::Equal)
        } else {
            self.entries.partial_cmp(&other.entries)
        }
    }
}

impl Hash for Mapping {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let mut entry_hashes = Vec::with_capacity(self.len());
        for entry in &self.entries {
            let mut hasher = DefaultHasher::new();
            entry.hash(&mut hasher);
            entry_hashes.push(hasher.finish());
        }
        entry_hashes.sort_unstable();
        self.len().hash(state);
        entry_hashes.hash(state);
    }
}

impl Serialize for Mapping {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut mapping = serializer.serialize_map(Some(self.len()))?;
        for (key, value) in self {
            mapping.serialize_entry(key, value)?;
        }
        mapping.end()
    }
}

impl<'de> Deserialize<'de> for Mapping {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct MappingVisitor;

        impl<'de> Visitor<'de> for MappingVisitor {
            type Value = Mapping;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a YAML mapping")
            }

            fn visit_map<A>(self, mut access: A) -> Result<Mapping, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut mapping = Mapping::with_capacity(access.size_hint().unwrap_or(0));
                while let Some((key, value)) = access.next_entry()? {
                    if mapping.insert(key, value).is_some() {
                        return Err(A::Error::custom("duplicate mapping key"));
                    }
                }
                Ok(mapping)
            }
        }

        deserializer.deserialize_map(MappingVisitor)
    }
}

impl Extend<(Value, Value)> for Mapping {
    fn extend<T>(&mut self, iter: T)
    where
        T: IntoIterator<Item = (Value, Value)>,
    {
        for (key, value) in iter {
            self.insert(key, value);
        }
    }
}

impl FromIterator<(Value, Value)> for Mapping {
    fn from_iter<T>(iter: T) -> Self
    where
        T: IntoIterator<Item = (Value, Value)>,
    {
        let mut mapping = Self::new();
        mapping.extend(iter);
        mapping
    }
}

impl<I> ops::Index<I> for Mapping
where
    I: Index,
{
    type Output = Value;

    fn index(&self, index: I) -> &Self::Output {
        self.get(index).expect("no entry found for key")
    }
}

impl<I> ops::IndexMut<I> for Mapping
where
    I: Index,
{
    fn index_mut(&mut self, index: I) -> &mut Self::Output {
        self.get_mut(index).expect("no entry found for key")
    }
}

/// A view into an occupied or vacant mapping entry.
pub enum Entry<'a> {
    /// An existing entry.
    Occupied(OccupiedEntry<'a>),
    /// A missing entry.
    Vacant(VacantEntry<'a>),
}

impl<'a> Entry<'a> {
    /// Returns the entry's key.
    #[must_use]
    pub fn key(&self) -> &Value {
        match self {
            Self::Occupied(entry) => entry.key(),
            Self::Vacant(entry) => entry.key(),
        }
    }

    /// Ensures a value is present and returns a mutable reference to it.
    pub fn or_insert(self, default: Value) -> &'a mut Value {
        match self {
            Self::Occupied(entry) => entry.into_mut(),
            Self::Vacant(entry) => entry.insert(default),
        }
    }

    /// Ensures a lazily produced value is present.
    pub fn or_insert_with<F>(self, default: F) -> &'a mut Value
    where
        F: FnOnce() -> Value,
    {
        match self {
            Self::Occupied(entry) => entry.into_mut(),
            Self::Vacant(entry) => entry.insert(default()),
        }
    }

    /// Modifies an occupied entry before returning it.
    pub fn and_modify<F>(mut self, modify: F) -> Self
    where
        F: FnOnce(&mut Value),
    {
        if let Self::Occupied(entry) = &mut self {
            modify(entry.get_mut());
        }
        self
    }
}

/// An occupied mapping entry.
pub struct OccupiedEntry<'a> {
    mapping: &'a mut Mapping,
    index: usize,
}

impl<'a> OccupiedEntry<'a> {
    /// Returns the entry's key.
    #[must_use]
    pub fn key(&self) -> &Value {
        &self.mapping.entries[self.index].0
    }

    /// Returns the entry's value.
    #[must_use]
    pub fn get(&self) -> &Value {
        &self.mapping.entries[self.index].1
    }

    /// Returns a mutable entry value.
    pub fn get_mut(&mut self) -> &mut Value {
        &mut self.mapping.entries[self.index].1
    }

    /// Converts this entry into a mutable value reference.
    pub fn into_mut(self) -> &'a mut Value {
        &mut self.mapping.entries[self.index].1
    }

    /// Replaces and returns the old value.
    pub fn insert(&mut self, value: Value) -> Value {
        std::mem::replace(self.get_mut(), value)
    }

    /// Removes and returns the value using swap removal.
    pub fn remove(self) -> Value {
        self.remove_entry().1
    }

    /// Removes and returns the entry using swap removal.
    pub fn remove_entry(self) -> (Value, Value) {
        self.mapping.entries.swap_remove(self.index)
    }
}

/// A vacant mapping entry.
pub struct VacantEntry<'a> {
    mapping: &'a mut Mapping,
    key: Value,
}

impl<'a> VacantEntry<'a> {
    /// Returns the entry's key.
    #[must_use]
    pub fn key(&self) -> &Value {
        &self.key
    }

    /// Consumes and returns the entry's key.
    #[must_use]
    pub fn into_key(self) -> Value {
        self.key
    }

    /// Inserts a value and returns a mutable reference to it.
    pub fn insert(self, value: Value) -> &'a mut Value {
        self.mapping.entries.push((self.key, value));
        &mut self
            .mapping
            .entries
            .last_mut()
            .expect("entry was inserted")
            .1
    }
}

/// Immutable mapping iterator.
#[derive(Clone)]
pub struct Iter<'a>(std::slice::Iter<'a, (Value, Value)>);

impl<'a> Iterator for Iter<'a> {
    type Item = (&'a Value, &'a Value);

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|(key, value)| (key, value))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl DoubleEndedIterator for Iter<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.0.next_back().map(|(key, value)| (key, value))
    }
}

impl ExactSizeIterator for Iter<'_> {}
impl FusedIterator for Iter<'_> {}

/// Mutable mapping iterator.
pub struct IterMut<'a>(std::slice::IterMut<'a, (Value, Value)>);

impl<'a> Iterator for IterMut<'a> {
    type Item = (&'a Value, &'a mut Value);

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|(key, value)| (&*key, value))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl DoubleEndedIterator for IterMut<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.0.next_back().map(|(key, value)| (&*key, value))
    }
}

impl ExactSizeIterator for IterMut<'_> {}
impl FusedIterator for IterMut<'_> {}

macro_rules! iterator_wrapper {
    ($name:ident, $inner:ty, $item:ty, $map:expr) => {
        pub struct $name<'a>($inner);

        impl<'a> Iterator for $name<'a> {
            type Item = $item;

            fn next(&mut self) -> Option<Self::Item> {
                self.0.next().map($map)
            }

            fn size_hint(&self) -> (usize, Option<usize>) {
                self.0.size_hint()
            }
        }

        impl DoubleEndedIterator for $name<'_> {
            fn next_back(&mut self) -> Option<Self::Item> {
                self.0.next_back().map($map)
            }
        }

        impl ExactSizeIterator for $name<'_> {}
        impl FusedIterator for $name<'_> {}
    };
}

iterator_wrapper!(Keys, Iter<'a>, &'a Value, |(key, _)| key);
iterator_wrapper!(Values, Iter<'a>, &'a Value, |(_, value)| value);
iterator_wrapper!(ValuesMut, IterMut<'a>, &'a mut Value, |(_, value)| value);

/// Owning mapping iterator.
pub type IntoIter = std::vec::IntoIter<(Value, Value)>;

/// Owning key iterator.
pub struct IntoKeys(IntoIter);

impl Iterator for IntoKeys {
    type Item = Value;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|(key, _)| key)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl DoubleEndedIterator for IntoKeys {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.0.next_back().map(|(key, _)| key)
    }
}

impl ExactSizeIterator for IntoKeys {}
impl FusedIterator for IntoKeys {}

/// Owning value iterator.
pub struct IntoValues(IntoIter);

impl Iterator for IntoValues {
    type Item = Value;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|(_, value)| value)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl DoubleEndedIterator for IntoValues {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.0.next_back().map(|(_, value)| value)
    }
}

impl ExactSizeIterator for IntoValues {}
impl FusedIterator for IntoValues {}

impl IntoIterator for Mapping {
    type Item = (Value, Value);
    type IntoIter = IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}

impl<'a> IntoIterator for &'a Mapping {
    type Item = (&'a Value, &'a Value);
    type IntoIter = Iter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a> IntoIterator for &'a mut Mapping {
    type Item = (&'a Value, &'a mut Value);
    type IntoIter = IterMut<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}
