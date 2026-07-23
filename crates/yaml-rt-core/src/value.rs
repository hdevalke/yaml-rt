use std::collections::{BTreeMap, HashSet};
use std::fmt;

use crate::{CollectionStyle, NodeId, SemanticKind, YamlDoc, YamlScalarStyle};

const NULL_TAG: &str = "tag:yaml.org,2002:null";
const BOOL_TAG: &str = "tag:yaml.org,2002:bool";
const INT_TAG: &str = "tag:yaml.org,2002:int";
const FLOAT_TAG: &str = "tag:yaml.org,2002:float";
const STR_TAG: &str = "tag:yaml.org,2002:str";
const SEQ_TAG: &str = "tag:yaml.org,2002:seq";
const MAP_TAG: &str = "tag:yaml.org,2002:map";

/// An exact, finite YAML number normalized for semantic comparison.
#[derive(Debug, Clone)]
pub struct YamlNumber {
    negative: bool,
    digits: String,
    exponent: i64,
    integer_syntax: bool,
}

impl YamlNumber {
    /// Returns whether the source used YAML integer rather than float syntax.
    #[must_use]
    pub const fn has_integer_syntax(&self) -> bool {
        self.integer_syntax
    }

    /// Converts an integer-syntax number to `i128` when it fits.
    #[must_use]
    pub fn as_i128(&self) -> Option<i128> {
        if !self.integer_syntax || self.exponent < 0 {
            return None;
        }
        let mut text = self.digits.clone();
        text.extend(std::iter::repeat_n(
            '0',
            usize::try_from(self.exponent).ok()?,
        ));
        if self.negative {
            text.insert(0, '-');
        }
        text.parse().ok()
    }

    /// Converts a non-negative integer-syntax number to `u128` when it fits.
    #[must_use]
    pub fn as_u128(&self) -> Option<u128> {
        if self.negative || !self.integer_syntax || self.exponent < 0 {
            return None;
        }
        let mut text = self.digits.clone();
        text.extend(std::iter::repeat_n(
            '0',
            usize::try_from(self.exponent).ok()?,
        ));
        text.parse().ok()
    }

    /// Converts this finite number to an `f64`.
    #[must_use]
    pub fn as_f64(&self) -> Option<f64> {
        let sign = if self.negative { "-" } else { "" };
        format!("{sign}{}e{}", self.digits, self.exponent)
            .parse()
            .ok()
    }
}

impl PartialEq for YamlNumber {
    fn eq(&self, other: &Self) -> bool {
        self.negative == other.negative
            && self.digits == other.digits
            && self.exponent == other.exponent
    }
}

impl Eq for YamlNumber {}

/// A non-finite YAML float, which is outside the JSON-compatible data model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NonFiniteFloat {
    /// Positive infinity.
    PositiveInfinity,
    /// Negative infinity.
    NegativeInfinity,
    /// Not a number.
    NaN,
}

/// YAML 1.2 core-schema interpretation of a scalar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedScalar {
    /// YAML null.
    Null,
    /// YAML boolean.
    Bool(bool),
    /// A finite integer or float.
    Number(YamlNumber),
    /// A YAML infinity or NaN spelling.
    NonFinite(NonFiniteFloat),
    /// A string scalar.
    String,
}

/// Failure to resolve a scalar according to the YAML core schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScalarResolveError {
    message: String,
}

impl ScalarResolveError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ScalarResolveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ScalarResolveError {}

/// Resolves a decoded scalar value using the YAML 1.2 core schema.
pub fn resolve_scalar(
    value: &str,
    style: YamlScalarStyle,
    tag: Option<&str>,
) -> Result<ResolvedScalar, ScalarResolveError> {
    if tag == Some(STR_TAG) {
        return Ok(ResolvedScalar::String);
    }
    if let Some(tag) = tag
        && !matches!(tag, NULL_TAG | BOOL_TAG | INT_TAG | FLOAT_TAG)
    {
        return Err(ScalarResolveError::new(format!(
            "unsupported scalar tag `{tag}`"
        )));
    }
    if style != YamlScalarStyle::Plain && tag.is_none() {
        return Ok(ResolvedScalar::String);
    }
    if tag == Some(NULL_TAG)
        || tag.is_none() && matches!(value, "" | "~" | "null" | "Null" | "NULL")
    {
        return if matches!(value, "" | "~" | "null" | "Null" | "NULL") {
            Ok(ResolvedScalar::Null)
        } else {
            Err(ScalarResolveError::new("invalid null scalar"))
        };
    }
    if tag == Some(BOOL_TAG) || tag.is_none() {
        match value {
            "true" | "True" | "TRUE" => return Ok(ResolvedScalar::Bool(true)),
            "false" | "False" | "FALSE" => return Ok(ResolvedScalar::Bool(false)),
            _ if tag == Some(BOOL_TAG) => {
                return Err(ScalarResolveError::new("invalid boolean scalar"));
            }
            _ => {}
        }
    }
    if tag == Some(INT_TAG) || tag.is_none() {
        if let Some(number) = parse_integer(value) {
            return Ok(ResolvedScalar::Number(number));
        }
        if tag == Some(INT_TAG) {
            return Err(ScalarResolveError::new("invalid integer scalar"));
        }
    }
    if tag == Some(FLOAT_TAG) || tag.is_none() && looks_like_float(value) {
        if let Some(special) = parse_non_finite(value) {
            return Ok(ResolvedScalar::NonFinite(special));
        }
        if let Some(number) = parse_decimal(value, false) {
            return Ok(ResolvedScalar::Number(number));
        }
        if tag == Some(FLOAT_TAG) {
            return Err(ScalarResolveError::new("invalid float scalar"));
        }
    }
    Ok(ResolvedScalar::String)
}

fn parse_integer(value: &str) -> Option<YamlNumber> {
    let normalized = value.replace('_', "");
    let (negative, unsigned) = strip_sign(&normalized);
    let (radix, digits) = if let Some(rest) = unsigned.strip_prefix("0x") {
        (16, rest)
    } else if let Some(rest) = unsigned.strip_prefix("0o") {
        (8, rest)
    } else if let Some(rest) = unsigned.strip_prefix("0b") {
        (2, rest)
    } else {
        (10, unsigned)
    };
    if digits.is_empty()
        || !digits.chars().all(|character| character.is_digit(radix))
        || radix == 10 && digits.len() > 1 && digits.starts_with('0')
    {
        return None;
    }
    let decimal = if radix == 10 {
        digits.to_owned()
    } else {
        radix_to_decimal(digits, radix)?
    };
    normalize_number(negative, decimal, 0, true)
}

fn radix_to_decimal(digits: &str, radix: u32) -> Option<String> {
    let mut decimal = vec![0_u8];
    for character in digits.chars() {
        let digit = character.to_digit(radix)?;
        let mut carry = digit;
        for value in decimal.iter_mut().rev() {
            let next = u32::from(*value) * radix + carry;
            *value = u8::try_from(next % 10).ok()?;
            carry = next / 10;
        }
        while carry > 0 {
            decimal.insert(0, u8::try_from(carry % 10).ok()?);
            carry /= 10;
        }
    }
    Some(
        decimal
            .into_iter()
            .map(|digit| char::from(b'0' + digit))
            .collect(),
    )
}

fn parse_decimal(value: &str, integer_syntax: bool) -> Option<YamlNumber> {
    let normalized = value.replace('_', "");
    let (negative, unsigned) = strip_sign(&normalized);
    let (mantissa, exponent) = match unsigned.find(['e', 'E']) {
        Some(index) => {
            let exponent = unsigned[index + 1..].parse::<i64>().ok()?;
            (&unsigned[..index], exponent)
        }
        None => (unsigned, 0),
    };
    let (whole, fraction) = match mantissa.split_once('.') {
        Some(parts) => parts,
        None => (mantissa, ""),
    };
    if whole.is_empty() && fraction.is_empty()
        || !whole.chars().all(|character| character.is_ascii_digit())
        || !fraction.chars().all(|character| character.is_ascii_digit())
    {
        return None;
    }
    let digits = format!("{whole}{fraction}");
    let exponent = exponent.checked_sub(i64::try_from(fraction.len()).ok()?)?;
    normalize_number(negative, digits, exponent, integer_syntax)
}

fn normalize_number(
    mut negative: bool,
    digits: String,
    mut exponent: i64,
    integer_syntax: bool,
) -> Option<YamlNumber> {
    let mut digits = digits.trim_start_matches('0').to_owned();
    if digits.is_empty() {
        negative = false;
        digits.push('0');
        exponent = 0;
    } else {
        while digits.ends_with('0') {
            digits.pop();
            exponent = exponent.checked_add(1)?;
        }
    }
    Some(YamlNumber {
        negative,
        digits,
        exponent,
        integer_syntax,
    })
}

fn strip_sign(value: &str) -> (bool, &str) {
    if let Some(rest) = value.strip_prefix('-') {
        (true, rest)
    } else {
        (false, value.strip_prefix('+').unwrap_or(value))
    }
}

fn looks_like_float(value: &str) -> bool {
    value.contains(['.', 'e', 'E'])
}

fn parse_non_finite(value: &str) -> Option<NonFiniteFloat> {
    match value {
        ".inf" | ".Inf" | ".INF" | "+.inf" | "+.Inf" | "+.INF" => {
            Some(NonFiniteFloat::PositiveInfinity)
        }
        "-.inf" | "-.Inf" | "-.INF" => Some(NonFiniteFloat::NegativeInfinity),
        ".nan" | ".NaN" | ".NAN" => Some(NonFiniteFloat::NaN),
        _ => None,
    }
}

/// Failure while projecting YAML nodes into the JSON-compatible data model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticValueError {
    message: String,
}

impl SemanticValueError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for SemanticValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SemanticValueError {}

/// Compares two YAML nodes using RFC 6902 JSON-value equality.
pub fn semantically_equal(
    left_doc: &YamlDoc,
    left: NodeId,
    right_doc: &YamlDoc,
    right: NodeId,
) -> Result<bool, SemanticValueError> {
    let mut active = HashSet::new();
    compare_nodes(left_doc, left, right_doc, right, &mut active, 0)
}

fn compare_nodes(
    left_doc: &YamlDoc,
    left: NodeId,
    right_doc: &YamlDoc,
    right: NodeId,
    active: &mut HashSet<(NodeId, NodeId)>,
    depth: usize,
) -> Result<bool, SemanticValueError> {
    if depth > 1024 {
        return Err(SemanticValueError::new(
            "semantic comparison recursion limit exceeded",
        ));
    }
    let left = resolve_alias_chain(left_doc, left)?;
    let right = resolve_alias_chain(right_doc, right)?;
    if !active.insert((left, right)) {
        return Err(SemanticValueError::new(
            "cyclic YAML values are not JSON-compatible",
        ));
    }
    let result = compare_resolved(left_doc, left, right_doc, right, active, depth);
    active.remove(&(left, right));
    result
}

fn compare_resolved(
    left_doc: &YamlDoc,
    left: NodeId,
    right_doc: &YamlDoc,
    right: NodeId,
    active: &mut HashSet<(NodeId, NodeId)>,
    depth: usize,
) -> Result<bool, SemanticValueError> {
    match (left_doc.semantic_kind(left), right_doc.semantic_kind(right)) {
        (
            Some(SemanticKind::Scalar { style: left_style }),
            Some(SemanticKind::Scalar { style: right_style }),
        ) => {
            let left = resolved_scalar_at(left_doc, left, left_style)?;
            let right = resolved_scalar_at(right_doc, right, right_style)?;
            if matches!(left, ResolvedScalar::NonFinite(_))
                || matches!(right, ResolvedScalar::NonFinite(_))
            {
                return Err(SemanticValueError::new(
                    "infinities and NaN are not JSON-compatible",
                ));
            }
            Ok(left == right)
        }
        (
            Some(SemanticKind::Sequence { style: left_style }),
            Some(SemanticKind::Sequence { style: right_style }),
        ) => {
            validate_collection_tag(left_doc, left, left_style, false)?;
            validate_collection_tag(right_doc, right, right_style, false)?;
            let left_items = left_doc.sequence_items(left).collect::<Vec<_>>();
            let right_items = right_doc.sequence_items(right).collect::<Vec<_>>();
            if left_items.len() != right_items.len() {
                return Ok(false);
            }
            for (left_item, right_item) in left_items.into_iter().zip(right_items) {
                if !compare_nodes(
                    left_doc,
                    left_item,
                    right_doc,
                    right_item,
                    active,
                    depth + 1,
                )? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        (
            Some(SemanticKind::Mapping { style: left_style }),
            Some(SemanticKind::Mapping { style: right_style }),
        ) => {
            validate_collection_tag(left_doc, left, left_style, true)?;
            validate_collection_tag(right_doc, right, right_style, true)?;
            let left_entries = json_mapping(left_doc, left)?;
            let right_entries = json_mapping(right_doc, right)?;
            if left_entries.len() != right_entries.len() {
                return Ok(false);
            }
            for (key, left_value) in left_entries {
                let Some(right_value) = right_entries.get(&key).copied() else {
                    return Ok(false);
                };
                if !compare_nodes(
                    left_doc,
                    left_value,
                    right_doc,
                    right_value,
                    active,
                    depth + 1,
                )? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        (Some(SemanticKind::Alias), _) | (_, Some(SemanticKind::Alias)) => {
            unreachable!("aliases are resolved before comparison")
        }
        (Some(_), Some(_)) => Ok(false),
        _ => Err(SemanticValueError::new("unknown semantic YAML node")),
    }
}

fn resolved_scalar_at(
    doc: &YamlDoc,
    node: NodeId,
    style: YamlScalarStyle,
) -> Result<ResolvedScalar, SemanticValueError> {
    let value = doc
        .scalar_value(node)
        .map_err(|error| SemanticValueError::new(error.to_string()))?;
    let tag = doc
        .resolved_tag(node)
        .map_err(|error| SemanticValueError::new(error.to_string()))?;
    resolve_scalar(&value, style, tag.as_deref())
        .map_err(|error| SemanticValueError::new(error.to_string()))
}

fn resolve_alias_chain(doc: &YamlDoc, mut node: NodeId) -> Result<NodeId, SemanticValueError> {
    let mut seen = HashSet::new();
    while matches!(doc.semantic_kind(node), Some(SemanticKind::Alias)) {
        if !seen.insert(node) {
            return Err(SemanticValueError::new("cyclic alias chain"));
        }
        node = doc.resolve_alias(node).ok_or_else(|| {
            SemanticValueError::new(format!(
                "unresolved alias `*{}`",
                doc.alias_name(node).unwrap_or_default()
            ))
        })?;
    }
    Ok(node)
}

fn json_mapping(
    doc: &YamlDoc,
    mapping: NodeId,
) -> Result<BTreeMap<String, NodeId>, SemanticValueError> {
    let mut entries = BTreeMap::new();
    for (key, value) in doc.mapping_entries(mapping) {
        let key = resolve_alias_chain(doc, key)?;
        let Some(SemanticKind::Scalar { style }) = doc.semantic_kind(key) else {
            return Err(SemanticValueError::new("mapping contains a non-string key"));
        };
        if resolved_scalar_at(doc, key, style)? != ResolvedScalar::String {
            return Err(SemanticValueError::new("mapping contains a non-string key"));
        }
        let key = doc
            .scalar_value(key)
            .map_err(|error| SemanticValueError::new(error.to_string()))?
            .into_owned();
        if entries.insert(key.clone(), value).is_some() {
            return Err(SemanticValueError::new(format!(
                "mapping contains duplicate key `{key}`"
            )));
        }
    }
    Ok(entries)
}

fn validate_collection_tag(
    doc: &YamlDoc,
    node: NodeId,
    _style: CollectionStyle,
    mapping: bool,
) -> Result<(), SemanticValueError> {
    let tag = doc
        .resolved_tag(node)
        .map_err(|error| SemanticValueError::new(error.to_string()))?;
    let expected = if mapping { MAP_TAG } else { SEQ_TAG };
    if tag.as_deref().is_some_and(|tag| tag != expected) {
        return Err(SemanticValueError::new(format!(
            "custom-tagged collections are not JSON-compatible: `{}`",
            tag.as_deref().unwrap_or_default()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_numbers_compare_across_yaml_spellings() {
        let values = ["1", "1.0", "1e0", "0x1"];
        let numbers = values
            .into_iter()
            .map(|value| {
                let ResolvedScalar::Number(number) =
                    resolve_scalar(value, YamlScalarStyle::Plain, None).unwrap()
                else {
                    panic!("expected number");
                };
                number
            })
            .collect::<Vec<_>>();
        assert!(numbers.windows(2).all(|pair| pair[0] == pair[1]));
    }

    #[test]
    fn semantic_equality_ignores_presentation_and_mapping_order() {
        let left = YamlDoc::parse("a: 1\nb: ['x', true]\n").unwrap();
        let right = YamlDoc::parse("{b: [x, TRUE], a: 1.0}\n").unwrap();
        assert!(
            semantically_equal(
                &left,
                left.document_root(0).unwrap().unwrap(),
                &right,
                right.document_root(0).unwrap().unwrap()
            )
            .unwrap()
        );
    }

    #[test]
    fn semantic_equality_rejects_non_string_keys() {
        let left = YamlDoc::parse("1: value\n").unwrap();
        let right = YamlDoc::parse("'1': value\n").unwrap();
        assert!(
            semantically_equal(
                &left,
                left.document_root(0).unwrap().unwrap(),
                &right,
                right.document_root(0).unwrap().unwrap()
            )
            .unwrap_err()
            .to_string()
            .contains("non-string")
        );
    }
}
