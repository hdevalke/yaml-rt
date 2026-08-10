use std::{borrow::Cow, cell::Cell, io::Read, rc::Rc, str};

use serde::Deserializer as _;
use serde::de::{
    self, DeserializeOwned, DeserializeSeed, EnumAccess, MapAccess, SeqAccess, VariantAccess,
    Visitor,
};
use yaml_rt_core::{
    NodeId, NonFiniteFloat, ResolvedScalar, SemanticKind, Span, YamlDoc, YamlScalarStyle,
    resolve_scalar,
};

use crate::{Error, Result};

const MAX_DEPTH: u8 = 128;

struct Input<'de> {
    doc: Option<YamlDoc>,
    borrowed: Option<&'de str>,
    error: Option<Error>,
    semantic_len: usize,
}

impl<'de> Input<'de> {
    fn parsed(text: String, borrowed: Option<&'de str>) -> Self {
        match YamlDoc::parse_owned(text) {
            Ok(doc) => {
                let semantic_len = doc.events().count().max(1);
                Self {
                    doc: Some(doc),
                    borrowed,
                    error: None,
                    semantic_len,
                }
            }
            Err(error) => Self {
                doc: None,
                borrowed,
                error: Some(error.into()),
                semantic_len: 1,
            },
        }
    }

    fn failed(error: Error) -> Self {
        Self {
            doc: None,
            borrowed: None,
            error: Some(error),
            semantic_len: 1,
        }
    }
}

/// A Serde deserializer over one YAML stream.
pub struct Deserializer<'de> {
    input: Rc<Input<'de>>,
    selected: Option<usize>,
    next_index: usize,
    yielded_error: bool,
}

impl<'de> Deserializer<'de> {
    /// Creates a deserializer borrowing a UTF-8 YAML string.
    #[must_use]
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(input: &'de str) -> Self {
        Self::new(Input::parsed(input.to_owned(), Some(input)))
    }

    /// Creates a deserializer borrowing a UTF-8 YAML byte slice.
    #[must_use]
    pub fn from_slice(input: &'de [u8]) -> Self {
        match str::from_utf8(input) {
            Ok(input) => Self::from_str(input),
            Err(error) => Self::new(Input::failed(Error::message(error.to_string()))),
        }
    }

    fn new(input: Input<'de>) -> Self {
        Self {
            input: Rc::new(input),
            selected: None,
            next_index: 0,
            yielded_error: false,
        }
    }

    fn node_deserializer(&self) -> Result<NodeDeserializer<'_, 'de>> {
        if let Some(error) = &self.input.error {
            return Err(error.clone());
        }
        let doc = self
            .input
            .doc
            .as_ref()
            .expect("parsed input has a document");
        let index = if let Some(index) = self.selected {
            index
        } else {
            match doc.document_count() {
                0 => return Err(Error::message("EOF while parsing a value")),
                1 => 0,
                _ => {
                    return Err(Error::message(
                        "deserializing from YAML containing more than one document is not supported",
                    ));
                }
            }
        };
        let node = doc.document_root(index).map_err(Error::from)?;
        Ok(NodeDeserializer {
            input: &self.input,
            node,
            path: ".".to_owned(),
            depth: MAX_DEPTH,
            alias_jumps: Rc::new(Cell::new(0)),
            ignore_tag: false,
        })
    }
}

impl Deserializer<'static> {
    /// Creates a deserializer by reading an owned UTF-8 YAML stream.
    #[must_use]
    pub fn from_reader<R>(mut reader: R) -> Self
    where
        R: Read,
    {
        let mut input = String::new();
        match reader.read_to_string(&mut input) {
            Ok(_) => Self::new(Input::parsed(input, None)),
            Err(error) => Self::new(Input::failed(Error::io(error))),
        }
    }
}

impl<'de> Iterator for Deserializer<'de> {
    type Item = Self;

    fn next(&mut self) -> Option<Self::Item> {
        if self.input.error.is_some() {
            if self.yielded_error {
                return None;
            }
            self.yielded_error = true;
            return Some(Self {
                input: Rc::clone(&self.input),
                selected: Some(0),
                next_index: 0,
                yielded_error: true,
            });
        }
        let count = self.input.doc.as_ref()?.document_count();
        if self.next_index >= count {
            return None;
        }
        let selected = self.next_index;
        self.next_index += 1;
        Some(Self {
            input: Rc::clone(&self.input),
            selected: Some(selected),
            next_index: 0,
            yielded_error: false,
        })
    }
}

/// Deserializes exactly one YAML document from a string.
///
/// # Errors
///
/// Returns an error when the YAML is invalid, does not contain exactly one
/// document, or cannot be deserialized as `T`.
pub fn from_str<'de, T>(input: &'de str) -> Result<T>
where
    T: serde::Deserialize<'de>,
{
    T::deserialize(Deserializer::from_str(input))
}

/// Deserializes exactly one YAML document from a byte slice.
///
/// # Errors
///
/// Returns an error when the bytes are not UTF-8, the YAML is invalid, or the
/// document cannot be deserialized as `T`.
pub fn from_slice<'de, T>(input: &'de [u8]) -> Result<T>
where
    T: serde::Deserialize<'de>,
{
    T::deserialize(Deserializer::from_slice(input))
}

/// Deserializes exactly one owned YAML document from a reader.
///
/// # Errors
///
/// Returns an error when reading fails, the YAML is invalid, or the document
/// cannot be deserialized as `T`.
pub fn from_reader<R, T>(reader: R) -> Result<T>
where
    R: Read,
    T: DeserializeOwned,
{
    T::deserialize(Deserializer::from_reader(reader))
}

macro_rules! delegate_deserializer {
    ($($method:ident $(($($arg:ident : $ty:ty),*))?;)+) => {
        $(
            fn $method<V>(self, $($($arg: $ty,)*)? visitor: V) -> Result<V::Value>
            where
                V: Visitor<'de>,
            {
                self.node_deserializer()?.$method($($($arg,)*)? visitor)
            }
        )+
    };
}

impl<'de> de::Deserializer<'de> for Deserializer<'de> {
    type Error = Error;

    delegate_deserializer! {
        deserialize_any;
        deserialize_bool;
        deserialize_i8;
        deserialize_i16;
        deserialize_i32;
        deserialize_i64;
        deserialize_i128;
        deserialize_u8;
        deserialize_u16;
        deserialize_u32;
        deserialize_u64;
        deserialize_u128;
        deserialize_f32;
        deserialize_f64;
        deserialize_char;
        deserialize_str;
        deserialize_string;
        deserialize_bytes;
        deserialize_byte_buf;
        deserialize_option;
        deserialize_unit;
        deserialize_unit_struct(name: &'static str);
        deserialize_newtype_struct(name: &'static str);
        deserialize_seq;
        deserialize_tuple(len: usize);
        deserialize_tuple_struct(name: &'static str, len: usize);
        deserialize_map;
        deserialize_struct(name: &'static str, fields: &'static [&'static str]);
        deserialize_enum(name: &'static str, variants: &'static [&'static str]);
        deserialize_identifier;
        deserialize_ignored_any;
    }

    fn is_human_readable(&self) -> bool {
        true
    }
}

#[derive(Clone)]
struct NodeDeserializer<'input, 'de> {
    input: &'input Input<'de>,
    node: Option<NodeId>,
    path: String,
    depth: u8,
    alias_jumps: Rc<Cell<usize>>,
    ignore_tag: bool,
}

impl<'input, 'de> NodeDeserializer<'input, 'de> {
    fn doc(&self) -> &'input YamlDoc {
        self.input.doc.as_ref().expect("node input is parsed")
    }

    fn span(&self) -> Span {
        self.node
            .and_then(|node| self.doc().node(node).map(|node| node.span()))
            .unwrap_or_else(|| Span::empty(0))
    }

    fn annotate<T>(&self, result: Result<T>) -> Result<T> {
        result.map_err(|error| error.at(self.doc(), self.span()).with_path(&self.path))
    }

    fn descend(&self, node: Option<NodeId>, path: String) -> Result<Self> {
        let depth = self
            .depth
            .checked_sub(1)
            .ok_or_else(|| Error::message("recursion limit exceeded"));
        let depth = self.annotate(depth)?;
        Ok(Self {
            input: self.input,
            node,
            path,
            depth,
            alias_jumps: Rc::clone(&self.alias_jumps),
            ignore_tag: false,
        })
    }

    fn resolved(mut self) -> Result<Self> {
        let mut seen = Vec::new();
        while let Some(node) = self.node {
            if !matches!(self.doc().semantic_kind(node), Some(SemanticKind::Alias)) {
                break;
            }
            if seen.contains(&node) {
                return self.annotate(Err(Error::message("recursive alias")));
            }
            seen.push(node);
            let jumps = self.alias_jumps.get().saturating_add(1);
            self.alias_jumps.set(jumps);
            if jumps > self.input.semantic_len.saturating_mul(100) {
                return self.annotate(Err(Error::message("alias repetition limit exceeded")));
            }
            let target = self.doc().resolve_alias(node).ok_or_else(|| {
                Error::message(format!(
                    "unknown anchor `{}`",
                    self.doc().alias_name(node).unwrap_or_default()
                ))
            });
            self.node = Some(self.annotate(target)?);
        }
        Ok(self)
    }

    fn child(&self, node: Option<NodeId>, path: String) -> Result<Self> {
        self.descend(node, path)
    }

    fn scalar(&self) -> Result<(NodeId, Cow<'input, str>)> {
        let node = self
            .node
            .ok_or_else(|| Error::message("expected a scalar, found null"));
        let node = self.annotate(node)?;
        if !matches!(
            self.doc().semantic_kind(node),
            Some(SemanticKind::Scalar { .. })
        ) {
            return self.annotate(Err(Error::message("expected a scalar value")));
        }
        let value = self
            .doc()
            .scalar_value(node)
            .map(|value| (node, value))
            .map_err(Error::from);
        self.annotate(value)
    }

    fn custom_tag(&self) -> Option<String> {
        if self.ignore_tag {
            return None;
        }
        let raw = self.node.and_then(|node| self.doc().raw_tag(node))?;
        if raw.starts_with("!!") || raw.starts_with("!<") || !raw.starts_with('!') {
            return None;
        }
        let tag = raw.strip_prefix('!')?;
        (!tag.is_empty()).then(|| tag.to_owned())
    }

    fn scalar_kind(&self) -> Result<ScalarKind> {
        let Some(node) = self.node else {
            return Ok(ScalarKind::Null);
        };
        let style = match self.doc().semantic_kind(node) {
            Some(SemanticKind::Scalar { style }) => style,
            _ => return Err(Error::message("expected a scalar value")),
        };
        let value = self.doc().scalar_value(node).map_err(Error::from)?;
        let tag = self
            .doc()
            .resolved_tag(node)
            .map_err(Error::from)?
            .map(Cow::into_owned);
        let resolved = resolve_scalar(&value, style, tag.as_deref())
            .map_err(|error| Error::message(error.to_string()))?;
        self.annotate(scalar_kind_from_resolved(resolved))
    }

    fn borrowed_scalar(&self, node: NodeId) -> Option<&'de str> {
        let source = self.input.borrowed?;
        let span = self.doc().borrowable_scalar_span(node).ok()??;
        source.get(span.start as usize..span.end as usize)
    }

    fn is_empty_plain(&self) -> bool {
        let Some(node) = self.node else {
            return true;
        };
        matches!(
            self.doc().semantic_kind(node),
            Some(SemanticKind::Scalar {
                style: YamlScalarStyle::Plain
            })
        ) && self
            .doc()
            .scalar_value(node)
            .is_ok_and(|value| value.is_empty())
    }
}

enum ScalarKind {
    Null,
    Bool(bool),
    Signed(i128),
    Unsigned(u128),
    Float(f64),
    String,
}

fn scalar_kind_from_resolved(value: ResolvedScalar) -> Result<ScalarKind> {
    match value {
        ResolvedScalar::Null => Ok(ScalarKind::Null),
        ResolvedScalar::Bool(value) => Ok(ScalarKind::Bool(value)),
        ResolvedScalar::Number(number) if number.has_integer_syntax() => {
            if let Some(value) = number.as_i128()
                && value < 0
            {
                Ok(ScalarKind::Signed(value))
            } else if let Some(value) = number.as_u128() {
                Ok(ScalarKind::Unsigned(value))
            } else if let Some(value) = number.as_i128() {
                Ok(ScalarKind::Signed(value))
            } else {
                Err(Error::message("integer is outside the supported range"))
            }
        }
        ResolvedScalar::Number(number) => number
            .as_f64()
            .map(ScalarKind::Float)
            .ok_or_else(|| Error::message("float is outside the supported range")),
        ResolvedScalar::NonFinite(NonFiniteFloat::PositiveInfinity) => {
            Ok(ScalarKind::Float(f64::INFINITY))
        }
        ResolvedScalar::NonFinite(NonFiniteFloat::NegativeInfinity) => {
            Ok(ScalarKind::Float(f64::NEG_INFINITY))
        }
        ResolvedScalar::NonFinite(NonFiniteFloat::NaN) => Ok(ScalarKind::Float(f64::NAN)),
        ResolvedScalar::String => Ok(ScalarKind::String),
    }
}

impl<'de> de::Deserializer<'de> for NodeDeserializer<'_, 'de> {
    type Error = Error;

    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        let this = self.resolved()?;
        if let Some(variant) = this.custom_tag() {
            return visitor.visit_enum(YamlEnumAccess { de: this, variant });
        }
        let result = match this.node.and_then(|node| this.doc().semantic_kind(node)) {
            None => visitor.visit_unit(),
            Some(SemanticKind::Scalar { .. }) => match this.scalar_kind()? {
                ScalarKind::Null => visitor.visit_unit(),
                ScalarKind::Bool(value) => visitor.visit_bool(value),
                ScalarKind::Signed(value) => visitor.visit_i128(value),
                ScalarKind::Unsigned(value) => visitor.visit_u128(value),
                ScalarKind::Float(value) => visitor.visit_f64(value),
                ScalarKind::String => this.clone().deserialize_str(visitor),
            },
            Some(SemanticKind::Sequence { .. }) => this.clone().deserialize_seq(visitor),
            Some(SemanticKind::Mapping { .. }) => this.clone().deserialize_map(visitor),
            Some(SemanticKind::Alias) => unreachable!("aliases were resolved"),
            Some(SemanticKind::Document) => Err(Error::message("unexpected document node")),
        };
        this.annotate(result)
    }

    fn deserialize_bool<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        let this = self.resolved()?;
        let result = match this.scalar_kind()? {
            ScalarKind::Bool(value) => visitor.visit_bool(value),
            _ => Err(Error::message("expected a boolean")),
        };
        this.annotate(result)
    }

    fn deserialize_i8<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        deserialize_signed(self, visitor, |v| i8::try_from(v).ok(), Visitor::visit_i8)
    }
    fn deserialize_i16<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        deserialize_signed(self, visitor, |v| i16::try_from(v).ok(), Visitor::visit_i16)
    }
    fn deserialize_i32<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        deserialize_signed(self, visitor, |v| i32::try_from(v).ok(), Visitor::visit_i32)
    }
    fn deserialize_i64<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        deserialize_signed(self, visitor, |v| i64::try_from(v).ok(), Visitor::visit_i64)
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
        deserialize_unsigned(self, visitor, |v| u8::try_from(v).ok(), Visitor::visit_u8)
    }
    fn deserialize_u16<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        deserialize_unsigned(self, visitor, |v| u16::try_from(v).ok(), Visitor::visit_u16)
    }
    fn deserialize_u32<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        deserialize_unsigned(self, visitor, |v| u32::try_from(v).ok(), Visitor::visit_u32)
    }
    fn deserialize_u64<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        deserialize_unsigned(self, visitor, |v| u64::try_from(v).ok(), Visitor::visit_u64)
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
            visitor.visit_f32(value as f32)
        })
    }

    fn deserialize_f64<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        deserialize_float(self, visitor, Visitor::visit_f64)
    }

    fn deserialize_char<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        let this = self.resolved()?;
        let (_, value) = this.scalar()?;
        let mut chars = value.chars();
        let value = chars
            .next()
            .filter(|_| chars.next().is_none())
            .ok_or_else(|| Error::message("expected a single character"));
        this.annotate(value.and_then(|value| visitor.visit_char(value)))
    }

    fn deserialize_str<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        let this = self.resolved()?;
        let (node, value) = this.scalar()?;
        let result = if let Some(value) = this.borrowed_scalar(node) {
            visitor.visit_borrowed_str(value)
        } else {
            match value {
                Cow::Borrowed(value) => visitor.visit_str(value),
                Cow::Owned(value) => visitor.visit_string(value),
            }
        };
        this.annotate(result)
    }

    fn deserialize_string<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        let this = self.resolved()?;
        let (_, value) = this.scalar()?;
        this.annotate(visitor.visit_string(value.into_owned()))
    }

    fn deserialize_bytes<V>(self, _visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        Err(Error::message(
            "serialization and deserialization of bytes in YAML is not implemented",
        ))
    }

    fn deserialize_byte_buf<V>(self, _visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        Err(Error::message(
            "serialization and deserialization of bytes in YAML is not implemented",
        ))
    }

    fn deserialize_option<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        let this = self.resolved()?;
        if matches!(this.scalar_kind(), Ok(ScalarKind::Null)) {
            this.annotate(visitor.visit_none())
        } else {
            visitor.visit_some(this)
        }
    }

    fn deserialize_unit<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        let this = self.resolved()?;
        let result = if matches!(this.scalar_kind(), Ok(ScalarKind::Null)) {
            visitor.visit_unit()
        } else {
            Err(Error::message("expected null"))
        };
        this.annotate(result)
    }

    fn deserialize_unit_struct<V>(self, _name: &'static str, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        self.deserialize_unit(visitor)
    }

    fn deserialize_newtype_struct<V>(self, _name: &'static str, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        let this = self.resolved()?;
        if this.is_empty_plain() {
            return this.annotate(visitor.visit_seq(EmptyAccess));
        }
        let node = this
            .node
            .ok_or_else(|| Error::message("expected a sequence"))?;
        if !matches!(
            this.doc().semantic_kind(node),
            Some(SemanticKind::Sequence { .. })
        ) {
            return this.annotate(Err(Error::message("expected a sequence")));
        }
        let items = this.doc().sequence_items(node).collect::<Vec<_>>();
        let context = this.clone();
        context.annotate(visitor.visit_seq(YamlSeqAccess {
            de: this,
            items,
            index: 0,
        }))
    }

    fn deserialize_tuple<V>(self, _len: usize, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        self.deserialize_seq(visitor)
    }

    fn deserialize_tuple_struct<V>(
        self,
        _name: &'static str,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        self.deserialize_seq(visitor)
    }

    fn deserialize_map<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        let this = self.resolved()?;
        if this.is_empty_plain() {
            return this.annotate(visitor.visit_map(EmptyAccess));
        }
        let node = this
            .node
            .ok_or_else(|| Error::message("expected a mapping"))?;
        if !matches!(
            this.doc().semantic_kind(node),
            Some(SemanticKind::Mapping { .. })
        ) {
            return this.annotate(Err(Error::message("expected a mapping")));
        }
        let entries = this.doc().mapping_entries(node).collect::<Vec<_>>();
        let context = this.clone();
        context.annotate(visitor.visit_map(YamlMapAccess {
            de: this,
            entries,
            index: 0,
            pending: None,
        }))
    }

    fn deserialize_struct<V>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        self.deserialize_map(visitor)
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
        let this = self.resolved()?;
        let variant = if let Some(variant) = this.custom_tag() {
            variant
        } else {
            let (_, value) = this.scalar()?;
            value.into_owned()
        };
        let context = this.clone();
        context.annotate(visitor.visit_enum(YamlEnumAccess { de: this, variant }))
    }

    fn deserialize_identifier<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        self.deserialize_str(visitor)
    }

    fn deserialize_ignored_any<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        visitor.visit_unit()
    }

    fn is_human_readable(&self) -> bool {
        true
    }
}

fn signed_value(kind: ScalarKind) -> Option<i128> {
    match kind {
        ScalarKind::Signed(value) => Some(value),
        ScalarKind::Unsigned(value) => i128::try_from(value).ok(),
        _ => None,
    }
}

fn unsigned_value(kind: ScalarKind) -> Option<u128> {
    match kind {
        ScalarKind::Unsigned(value) => Some(value),
        ScalarKind::Signed(value) => u128::try_from(value).ok(),
        _ => None,
    }
}

fn deserialize_signed<'de, V, T>(
    de: NodeDeserializer<'_, 'de>,
    visitor: V,
    convert: impl FnOnce(i128) -> Option<T>,
    visit: impl FnOnce(V, T) -> Result<V::Value>,
) -> Result<V::Value>
where
    V: Visitor<'de>,
{
    let de = de.resolved()?;
    let value = signed_value(de.scalar_kind()?)
        .and_then(convert)
        .ok_or_else(|| Error::message("expected an integer in range"));
    de.annotate(value.and_then(|value| visit(visitor, value)))
}

fn deserialize_unsigned<'de, V, T>(
    de: NodeDeserializer<'_, 'de>,
    visitor: V,
    convert: impl FnOnce(u128) -> Option<T>,
    visit: impl FnOnce(V, T) -> Result<V::Value>,
) -> Result<V::Value>
where
    V: Visitor<'de>,
{
    let de = de.resolved()?;
    let value = unsigned_value(de.scalar_kind()?)
        .and_then(convert)
        .ok_or_else(|| Error::message("expected an unsigned integer in range"));
    de.annotate(value.and_then(|value| visit(visitor, value)))
}

fn deserialize_float<'de, V>(
    de: NodeDeserializer<'_, 'de>,
    visitor: V,
    visit: impl FnOnce(V, f64) -> Result<V::Value>,
) -> Result<V::Value>
where
    V: Visitor<'de>,
{
    let de = de.resolved()?;
    let value = match de.scalar_kind()? {
        ScalarKind::Float(value) => Some(value),
        ScalarKind::Signed(value) => Some(value as f64),
        ScalarKind::Unsigned(value) => Some(value as f64),
        _ => None,
    }
    .ok_or_else(|| Error::message("expected a number"));
    de.annotate(value.and_then(|value| visit(visitor, value)))
}

struct EmptyAccess;

impl<'de> SeqAccess<'de> for EmptyAccess {
    type Error = Error;
    fn next_element_seed<T>(&mut self, _seed: T) -> Result<Option<T::Value>>
    where
        T: DeserializeSeed<'de>,
    {
        Ok(None)
    }
}

impl<'de> MapAccess<'de> for EmptyAccess {
    type Error = Error;
    fn next_key_seed<K>(&mut self, _seed: K) -> Result<Option<K::Value>>
    where
        K: DeserializeSeed<'de>,
    {
        Ok(None)
    }
    fn next_value_seed<V>(&mut self, _seed: V) -> Result<V::Value>
    where
        V: DeserializeSeed<'de>,
    {
        Err(Error::message("value requested without a key"))
    }
}

struct YamlSeqAccess<'input, 'de> {
    de: NodeDeserializer<'input, 'de>,
    items: Vec<NodeId>,
    index: usize,
}

impl<'de> SeqAccess<'de> for YamlSeqAccess<'_, 'de> {
    type Error = Error;
    fn next_element_seed<T>(&mut self, seed: T) -> Result<Option<T::Value>>
    where
        T: DeserializeSeed<'de>,
    {
        let Some(node) = self.items.get(self.index).copied() else {
            return Ok(None);
        };
        let index = self.index;
        self.index += 1;
        let child = self
            .de
            .child(Some(node), format!("{}[{index}]", self.de.path))?;
        seed.deserialize(child).map(Some)
    }
    fn size_hint(&self) -> Option<usize> {
        Some(self.items.len().saturating_sub(self.index))
    }
}

struct YamlMapAccess<'input, 'de> {
    de: NodeDeserializer<'input, 'de>,
    entries: Vec<(NodeId, NodeId)>,
    index: usize,
    pending: Option<(NodeId, String)>,
}

impl<'de> MapAccess<'de> for YamlMapAccess<'_, 'de> {
    type Error = Error;
    fn next_key_seed<K>(&mut self, seed: K) -> Result<Option<K::Value>>
    where
        K: DeserializeSeed<'de>,
    {
        let Some((key, value)) = self.entries.get(self.index).copied() else {
            return Ok(None);
        };
        self.index += 1;
        let key_name = self
            .de
            .doc()
            .scalar_value(key)
            .ok()
            .map_or_else(|| "?".to_owned(), Cow::into_owned);
        let path = if key_name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        {
            format!("{}.{}", self.de.path.trim_end_matches('.'), key_name)
        } else {
            format!("{}[{:?}]", self.de.path, key_name)
        };
        self.pending = Some((value, path));
        let child = self
            .de
            .child(Some(key), format!("{}.<key>", self.de.path))?;
        seed.deserialize(child).map(Some)
    }
    fn next_value_seed<V>(&mut self, seed: V) -> Result<V::Value>
    where
        V: DeserializeSeed<'de>,
    {
        let (node, path) = self
            .pending
            .take()
            .ok_or_else(|| Error::message("value requested without a key"))?;
        seed.deserialize(self.de.child(Some(node), path)?)
    }
    fn size_hint(&self) -> Option<usize> {
        Some(self.entries.len().saturating_sub(self.index))
    }
}

struct YamlEnumAccess<'input, 'de> {
    de: NodeDeserializer<'input, 'de>,
    variant: String,
}

impl<'input, 'de> EnumAccess<'de> for YamlEnumAccess<'input, 'de> {
    type Error = Error;
    type Variant = YamlVariantAccess<'input, 'de>;
    fn variant_seed<V>(self, seed: V) -> Result<(V::Value, Self::Variant)>
    where
        V: DeserializeSeed<'de>,
    {
        let variant = seed.deserialize(serde::de::value::StrDeserializer::<Error>::new(
            self.variant.as_str(),
        ))?;
        Ok((variant, YamlVariantAccess { de: self.de }))
    }
}

struct YamlVariantAccess<'input, 'de> {
    de: NodeDeserializer<'input, 'de>,
}

impl<'de> VariantAccess<'de> for YamlVariantAccess<'_, 'de> {
    type Error = Error;
    fn unit_variant(self) -> Result<()> {
        Ok(())
    }
    fn newtype_variant_seed<T>(self, seed: T) -> Result<T::Value>
    where
        T: DeserializeSeed<'de>,
    {
        let mut de = self.de;
        de.ignore_tag = true;
        seed.deserialize(de)
    }
    fn tuple_variant<V>(self, len: usize, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        let mut de = self.de;
        de.ignore_tag = true;
        de.deserialize_tuple(len, visitor)
    }
    fn struct_variant<V>(self, fields: &'static [&'static str], visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        let mut de = self.de;
        de.ignore_tag = true;
        de.deserialize_struct("", fields, visitor)
    }
}
