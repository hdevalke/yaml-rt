#![allow(clippy::collapsible_if, clippy::too_many_arguments)]
use std::collections::{HashMap, HashSet};
#[cfg(not(target_arch = "wasm32"))]
use std::fs;

use crate::value::{Map, Value};
use iri_string::types::{IriReferenceStr, IriStr, UriReferenceStr, UriStr};
use regex::Regex;
use url::Url;

#[cfg(not(target_arch = "wasm32"))]
use crate::parse_schema;
use crate::{
    DIALECT, Error, Schema,
    model::{Instance, escape},
};

pub(crate) fn check_schema(schema: &Value, path: &str) -> Result<(), Error> {
    check_schema_inner(schema, path).map_err(|mut error| {
        if error.schema_path.is_none() {
            let keyword = error.message.split_whitespace().next().unwrap_or("");
            let suffix = if schema
                .as_object()
                .is_some_and(|object| object.contains_key(keyword))
            {
                format!("/{}", escape(keyword))
            } else {
                String::new()
            };
            error.schema_path = Some(format!("{path}{suffix}"));
        }
        error
    })
}

fn check_schema_inner(schema: &Value, path: &str) -> Result<(), Error> {
    if let Some(value) = schema.as_bool() {
        let _ = value;
        return Ok(());
    }
    let object = schema
        .as_object()
        .ok_or_else(|| Error::new(format!("schema at {path:?} must be an object or boolean")))?;
    if let Some(dialect) = object.get("$schema") {
        if dialect.as_str() != Some(DIALECT) {
            return Err(Error::new(format!(
                "unsupported JSON Schema dialect at {path:?}"
            )));
        }
    }
    if let Some(kind) = object.get("type") {
        let types = if let Some(kind) = kind.as_str() {
            vec![kind]
        } else {
            kind.as_array()
                .ok_or_else(|| Error::new("type must be a string or string array"))?
                .iter()
                .map(|v| {
                    v.as_str()
                        .ok_or_else(|| Error::new("type array must contain strings"))
                })
                .collect::<Result<Vec<_>, _>>()?
        };
        if types.is_empty()
            || types.iter().any(|kind| {
                !matches!(
                    *kind,
                    "null" | "boolean" | "object" | "array" | "number" | "integer" | "string"
                )
            })
        {
            return Err(Error::new("invalid type declaration"));
        }
    }
    for keyword in [
        "properties",
        "patternProperties",
        "$defs",
        "dependentSchemas",
    ] {
        if let Some(value) = object.get(keyword) {
            let children = value
                .as_object()
                .ok_or_else(|| Error::new(format!("{keyword} must be an object")))?;
            for (name, child) in children {
                check_schema(child, &format!("{path}/{keyword}/{}", escape(name)))?;
            }
        }
    }
    for keyword in [
        "items",
        "additionalItems",
        "additionalProperties",
        "unevaluatedItems",
        "unevaluatedProperties",
        "contains",
        "propertyNames",
        "not",
        "if",
        "then",
        "else",
        "contentSchema",
    ] {
        if let Some(child) = object.get(keyword) {
            check_schema(child, &format!("{path}/{keyword}"))?;
        }
    }
    for keyword in ["allOf", "anyOf", "oneOf", "prefixItems"] {
        if let Some(value) = object.get(keyword) {
            let children = value
                .as_array()
                .ok_or_else(|| Error::new(format!("{keyword} must be an array")))?;
            if children.is_empty() && keyword != "prefixItems" {
                return Err(Error::new(format!("{keyword} must not be empty")));
            }
            for (index, child) in children.iter().enumerate() {
                check_schema(child, &format!("{path}/{keyword}/{index}"))?;
            }
        }
    }
    for keyword in ["required", "dependentRequired"] {
        if let Some(value) = object.get(keyword) {
            if keyword == "required" {
                check_string_array(value, keyword)?;
            } else {
                for values in value
                    .as_object()
                    .ok_or_else(|| Error::new("dependentRequired must be an object"))?
                    .values()
                {
                    check_string_array(values, keyword)?;
                }
            }
        }
    }
    for keyword in [
        "minLength",
        "maxLength",
        "minItems",
        "maxItems",
        "minProperties",
        "maxProperties",
        "minContains",
        "maxContains",
    ] {
        if let Some(value) = object.get(keyword) {
            if !value.is_number()
                || !crate::numeric::is_integer(value)
                || crate::numeric::compare(value, &Value::Number(0.into()))
                    == Some(std::cmp::Ordering::Less)
            {
                return Err(Error::new(format!(
                    "{keyword} must be a non-negative integer"
                )));
            }
        }
    }
    for keyword in [
        "minimum",
        "maximum",
        "exclusiveMinimum",
        "exclusiveMaximum",
        "multipleOf",
    ] {
        if let Some(value) = object.get(keyword) {
            if !value.is_number()
                || keyword == "multipleOf"
                    && crate::numeric::compare(value, &Value::Number(0.into()))
                        != Some(std::cmp::Ordering::Greater)
            {
                return Err(Error::new(format!("{keyword} must be a valid number")));
            }
        }
    }
    for keyword in [
        "pattern",
        "$ref",
        "$dynamicRef",
        "$id",
        "$anchor",
        "$dynamicAnchor",
        "format",
        "contentEncoding",
        "contentMediaType",
    ] {
        if let Some(value) = object.get(keyword) {
            if !value.is_string() {
                return Err(Error::new(format!("{keyword} must be a string")));
            }
        }
    }
    for keyword in ["pattern", "patternProperties"] {
        if keyword == "pattern" {
            if let Some(pattern) = object.get(keyword).and_then(Value::as_str) {
                Regex::new(pattern).map_err(Error::source)?;
            }
        } else if let Some(patterns) = object.get(keyword).and_then(Value::as_object) {
            for pattern in patterns.keys() {
                Regex::new(pattern).map_err(Error::source)?;
            }
        }
    }
    for keyword in ["enum"] {
        if let Some(value) = object.get(keyword) {
            if value.as_array().is_none() {
                return Err(Error::new("enum must be an array"));
            }
        }
    }
    for keyword in ["uniqueItems", "readOnly", "writeOnly", "deprecated"] {
        if let Some(value) = object.get(keyword) {
            if !value.is_boolean() {
                return Err(Error::new(format!("{keyword} must be a boolean")));
            }
        }
    }
    Ok(())
}

pub(crate) fn check_meta_schema(schema: &Value) -> Result<(), Error> {
    let meta: Value = Value::parse(include_str!("../meta/schema.json"))
        .expect("bundled meta-schema is valid JSON");
    let validator = Schema {
        root: meta,
        origin: None,
        format_assertion: false,
        resources: Default::default(),
    };
    validator.validate_json(schema).map_err(|error| {
        let mut result = Error::new(format!("invalid 2020-12 schema: {error}"));
        result.schema_path = error.instance_path;
        result
    })
}

fn check_string_array(value: &Value, name: &str) -> Result<(), Error> {
    let values = value
        .as_array()
        .ok_or_else(|| Error::new(format!("{name} must be an array")))?;
    if values.iter().any(|v| !v.is_string()) {
        return Err(Error::new(format!("{name} must contain strings")));
    }
    Ok(())
}

#[derive(Default, Clone)]
struct Evaluated {
    properties: HashSet<String>,
    items: HashSet<usize>,
}
impl Evaluated {
    fn merge(&mut self, other: Self) {
        self.properties.extend(other.properties);
        self.items.extend(other.items);
    }
}

struct Validator<'a> {
    instance: &'a Instance,
    resources: HashMap<String, Value>,
    scope: Vec<String>,
    steps: usize,
    format_assertion: bool,
    root_base: String,
}

pub(crate) fn validate(schema: &Schema, instance: &Instance) -> Result<(), Error> {
    #[cfg(not(target_arch = "wasm32"))]
    let base = schema
        .origin
        .as_ref()
        .and_then(|path| Url::from_file_path(path.canonicalize().ok()?).ok())
        .unwrap_or_else(|| Url::parse("https://yaml-rt.invalid/root").unwrap());
    #[cfg(target_arch = "wasm32")]
    let base = Url::parse("https://yaml-rt.invalid/root").unwrap();
    let mut validator = Validator {
        instance,
        resources: HashMap::new(),
        scope: Vec::new(),
        steps: 0,
        format_assertion: schema.format_assertion,
        root_base: base.to_string(),
    };
    validator
        .resources
        .insert(base.to_string(), schema.root.clone());
    index_resources(&schema.root, &base, &mut validator.resources);
    for (uri, resource) in &schema.resources {
        let url = Url::parse(uri).map_err(Error::source)?;
        validator
            .resources
            .insert(url.to_string(), resource.clone());
        index_resources(resource, &url, &mut validator.resources);
    }
    for source in [
        include_str!("../meta/schema.json"),
        include_str!("../meta/core.json"),
        include_str!("../meta/applicator.json"),
        include_str!("../meta/unevaluated.json"),
        include_str!("../meta/validation.json"),
        include_str!("../meta/meta-data.json"),
        include_str!("../meta/format-annotation.json"),
        include_str!("../meta/content.json"),
    ] {
        let resource: Value = Value::parse(source).expect("bundled meta-schema is valid JSON");
        let id = resource
            .get("$id")
            .and_then(Value::as_str)
            .expect("bundled meta-schema has $id");
        let url = Url::parse(id).expect("bundled meta-schema $id is an absolute URI");
        validator
            .resources
            .insert(url.to_string(), resource.clone());
        index_resources(&resource, &url, &mut validator.resources);
    }
    validator.eval(&schema.root, &instance.value, "", "", Some(&base), 0)?;
    Ok(())
}

impl Validator<'_> {
    fn fail(&self, path: &str, schema_path: &str, message: impl Into<String>) -> Error {
        Error::keyword(
            path,
            schema_path,
            message,
            self.instance.spans.get(path).copied(),
        )
    }

    fn eval(
        &mut self,
        schema: &Value,
        value: &Value,
        path: &str,
        schema_path: &str,
        base: Option<&Url>,
        depth: usize,
    ) -> Result<Evaluated, Error> {
        let already_at_resource = base.is_some_and(|base| {
            base.to_string() != self.root_base
                && self.resources.get(&base.to_string()) == Some(schema)
        });
        let owned_base = if already_at_resource {
            None
        } else {
            schema
                .as_object()
                .and_then(|obj| obj.get("$id"))
                .and_then(Value::as_str)
                .and_then(|id| base.and_then(|base| base.join(id).ok()))
        };
        let base = owned_base.as_ref().or(base);
        let pushed = base.is_some_and(|base| self.scope.last() != Some(&base.to_string()));
        if pushed {
            self.scope.push(base.unwrap().to_string());
        }
        let result = self.eval_inner(schema, value, path, schema_path, base, depth);
        if pushed {
            self.scope.pop();
        }
        result
    }

    fn eval_inner(
        &mut self,
        schema: &Value,
        value: &Value,
        path: &str,
        schema_path: &str,
        base: Option<&Url>,
        depth: usize,
    ) -> Result<Evaluated, Error> {
        self.steps += 1;
        if depth > 128 || self.steps > 1_000_000 {
            return Err(self.fail(path, schema_path, "schema evaluation limit exceeded"));
        }
        if schema == &Value::Bool(true) {
            return Ok(Evaluated::default());
        }
        if schema == &Value::Bool(false) {
            return Err(self.fail(path, schema_path, "false schema rejects value"));
        }
        let obj = schema
            .as_object()
            .ok_or_else(|| self.fail(path, schema_path, "invalid schema"))?;
        let mut evaluated = Evaluated::default();
        if let Some(reference) = obj.get("$ref").and_then(Value::as_str) {
            let (target, target_path, target_base) = self.resolve(reference, base)?;
            evaluated.merge(self.eval(
                &target,
                value,
                path,
                &target_path,
                target_base.as_ref(),
                depth + 1,
            )?);
        }
        if let Some(reference) = obj.get("$dynamicRef").and_then(Value::as_str) {
            let (mut target, mut target_path, mut target_base) = self.resolve(reference, base)?;
            let anchor = reference
                .split_once('#')
                .map(|(_, fragment)| fragment)
                .unwrap_or("");
            if !anchor.is_empty()
                && !anchor.starts_with('/')
                && target.get("$dynamicAnchor").and_then(Value::as_str) == Some(anchor)
            {
                for uri in &self.scope {
                    if let Some(resource) = self.resources.get(uri)
                        && let Some(dynamic) = find_dynamic_anchor(resource, anchor)
                    {
                        target = dynamic.clone();
                        target_path = format!("#{anchor}");
                        target_base = Url::parse(uri).ok();
                        break;
                    }
                }
            }
            evaluated.merge(self.eval(
                &target,
                value,
                path,
                &target_path,
                target_base.as_ref(),
                depth + 1,
            )?);
        }
        if let Some(types) = obj.get("type") {
            let accepts = |kind: &str| match kind {
                "null" => value.is_null(),
                "boolean" => value.is_boolean(),
                "string" => value.is_string(),
                "array" => value.is_array(),
                "object" => value.is_object(),
                "number" => value.is_number(),
                "integer" => crate::numeric::is_integer(value),
                _ => false,
            };
            let valid = types.as_str().is_some_and(&accepts)
                || types.as_array().is_some_and(|types| {
                    types.iter().any(|kind| kind.as_str().is_some_and(&accepts))
                });
            if !valid {
                return Err(self.fail(path, &format!("{schema_path}/type"), "type mismatch"));
            }
        }
        if let Some(expected) = obj.get("const") {
            if !equal(value, expected) {
                return Err(self.fail(
                    path,
                    &format!("{schema_path}/const"),
                    "value differs from const",
                ));
            }
        }
        if let Some(options) = obj.get("enum").and_then(Value::as_array) {
            if !options.iter().any(|expected| equal(value, expected)) {
                return Err(self.fail(
                    path,
                    &format!("{schema_path}/enum"),
                    "value is not in enum",
                ));
            }
        }
        if value.is_number() {
            for keyword in [
                "minimum",
                "maximum",
                "exclusiveMinimum",
                "exclusiveMaximum",
                "multipleOf",
            ] {
                let Some(bound) = obj.get(keyword) else {
                    continue;
                };
                let matches = if keyword == "multipleOf" {
                    crate::numeric::multiple_of(value, bound).unwrap_or(false)
                } else {
                    let comparison = crate::numeric::compare(value, bound);
                    match keyword {
                        "minimum" => {
                            comparison.is_some_and(|order| order != std::cmp::Ordering::Less)
                        }
                        "maximum" => {
                            comparison.is_some_and(|order| order != std::cmp::Ordering::Greater)
                        }
                        "exclusiveMinimum" => {
                            comparison.is_some_and(|order| order == std::cmp::Ordering::Greater)
                        }
                        _ => comparison.is_some_and(|order| order == std::cmp::Ordering::Less),
                    }
                };
                if !matches {
                    return Err(self.fail(
                        path,
                        &format!("{schema_path}/{keyword}"),
                        format!("{keyword} constraint failed"),
                    ));
                }
            }
        }
        if let Some(text) = value.as_str() {
            let length = Value::Number(crate::value::Number::from_yaml(
                text.chars().count().to_string(),
            ));
            for (keyword, valid) in [
                (
                    "minLength",
                    obj.get("minLength")
                        .is_none_or(|n| count_at_least(&length, n)),
                ),
                (
                    "maxLength",
                    obj.get("maxLength")
                        .is_none_or(|n| count_at_most(&length, n)),
                ),
            ] {
                if !valid {
                    return Err(self.fail(
                        path,
                        &format!("{schema_path}/{keyword}"),
                        format!("{keyword} constraint failed"),
                    ));
                }
            }
            if let Some(pattern) = obj.get("pattern").and_then(Value::as_str) {
                if !Regex::new(pattern).map_err(Error::source)?.is_match(text) {
                    return Err(self.fail(
                        path,
                        &format!("{schema_path}/pattern"),
                        "pattern does not match",
                    ));
                }
            }
            if self.format_assertion
                && let Some(format) = obj.get("format").and_then(Value::as_str)
                && !format_valid(format, text)
            {
                return Err(self.fail(
                    path,
                    &format!("{schema_path}/format"),
                    format!("invalid {format} format"),
                ));
            }
        }
        for keyword in ["allOf", "anyOf", "oneOf"] {
            if let Some(branches) = obj.get(keyword).and_then(Value::as_array) {
                let mut passing = Vec::new();
                let mut first_error = None;
                for (index, branch) in branches.iter().enumerate() {
                    match self.eval(
                        branch,
                        value,
                        path,
                        &format!("{schema_path}/{keyword}/{index}"),
                        base,
                        depth + 1,
                    ) {
                        Ok(annotations) => passing.push(annotations),
                        Err(error) => {
                            if first_error.is_none() {
                                first_error = Some(error);
                            }
                        }
                    }
                }
                let valid = match keyword {
                    "allOf" => passing.len() == branches.len(),
                    "anyOf" => !passing.is_empty(),
                    _ => passing.len() == 1,
                };
                if !valid {
                    return Err(first_error.unwrap_or_else(|| {
                        self.fail(
                            path,
                            &format!("{schema_path}/{keyword}"),
                            format!("{keyword} failed"),
                        )
                    }));
                }
                for annotations in passing {
                    evaluated.merge(annotations);
                }
            }
        }
        if let Some(branch) = obj.get("not") {
            if self
                .eval(
                    branch,
                    value,
                    path,
                    &format!("{schema_path}/not"),
                    base,
                    depth + 1,
                )
                .is_ok()
            {
                return Err(self.fail(
                    path,
                    &format!("{schema_path}/not"),
                    "not constraint failed",
                ));
            }
        }
        if let Some(condition) = obj.get("if") {
            let matched = self.eval(
                condition,
                value,
                path,
                &format!("{schema_path}/if"),
                base,
                depth + 1,
            );
            let keyword = if matched.is_ok() { "then" } else { "else" };
            if let Some(branch) = obj.get(keyword) {
                evaluated.merge(self.eval(
                    branch,
                    value,
                    path,
                    &format!("{schema_path}/{keyword}"),
                    base,
                    depth + 1,
                )?);
            }
            if let Ok(annotations) = matched {
                evaluated.merge(annotations);
            }
        }
        if let Some(array) = value.as_array() {
            self.eval_array(obj, array, path, schema_path, base, depth, &mut evaluated)?;
        }
        if let Some(object) = value.as_object() {
            self.eval_object(obj, object, path, schema_path, base, depth, &mut evaluated)?;
        }
        Ok(evaluated)
    }

    fn resolve(
        &mut self,
        reference: &str,
        base: Option<&Url>,
    ) -> Result<(Value, String, Option<Url>), Error> {
        let base = base.ok_or_else(|| Error::new("schema base URI is unavailable"))?;
        let mut target_base = base.join(reference).map_err(Error::source)?;
        let fragment = percent_decode(target_base.fragment().unwrap_or_default())?;
        target_base.set_fragment(None);
        let key = target_base.to_string();
        if !self.resources.contains_key(&key) {
            #[cfg(target_arch = "wasm32")]
            return Err(Error::new(format!(
                "separate schema references are unavailable in the browser: {reference}"
            )));
            #[cfg(not(target_arch = "wasm32"))]
            {
                if target_base.scheme() != "file" {
                    return Err(Error::new(format!(
                        "remote schema reference is unsupported: {reference}"
                    )));
                }
                let path = target_base
                    .to_file_path()
                    .map_err(|_| Error::new("invalid file schema reference"))?;
                let source = fs::read_to_string(&path).map_err(Error::source)?;
                let document = parse_schema(&source, Some(&path))?;
                check_schema(&document, "")?;
                check_meta_schema(&document)?;
                self.resources.insert(key.clone(), document.clone());
                index_resources(&document, &target_base, &mut self.resources);
            }
        }
        let document = &self.resources[&key];
        if fragment.is_empty() {
            return Ok((document.clone(), "".into(), Some(target_base)));
        }
        if fragment.starts_with('/') {
            let target = document
                .pointer(&fragment)
                .ok_or_else(|| Error::new(format!("unresolved reference {reference}")))?;
            return Ok((target.clone(), fragment, Some(target_base)));
        }
        let target = find_anchor(document, &fragment)
            .ok_or_else(|| Error::new(format!("unresolved anchor {reference}")))?;
        Ok((target.clone(), format!("#{fragment}"), Some(target_base)))
    }

    fn eval_array(
        &mut self,
        obj: &Map,
        array: &[Value],
        path: &str,
        schema_path: &str,
        base: Option<&Url>,
        depth: usize,
        evaluated: &mut Evaluated,
    ) -> Result<(), Error> {
        let len = Value::Number(crate::value::Number::from_yaml(array.len().to_string()));
        for (keyword, valid) in [
            (
                "minItems",
                obj.get("minItems").is_none_or(|n| count_at_least(&len, n)),
            ),
            (
                "maxItems",
                obj.get("maxItems").is_none_or(|n| count_at_most(&len, n)),
            ),
        ] {
            if !valid {
                return Err(self.fail(
                    path,
                    &format!("{schema_path}/{keyword}"),
                    format!("{keyword} constraint failed"),
                ));
            }
        }
        if obj.get("uniqueItems") == Some(&Value::Bool(true)) {
            for i in 0..array.len() {
                for j in 0..i {
                    if equal(&array[i], &array[j]) {
                        return Err(self.fail(
                            &format!("{path}/{i}"),
                            &format!("{schema_path}/uniqueItems"),
                            "duplicate array item",
                        ));
                    }
                }
            }
        }
        let prefix = obj.get("prefixItems").and_then(Value::as_array);
        let prefix_len = prefix.map_or(0, Vec::len);
        if let Some(prefix) = prefix {
            for (index, schema) in prefix.iter().enumerate().take(array.len()) {
                self.eval(
                    schema,
                    &array[index],
                    &format!("{path}/{index}"),
                    &format!("{schema_path}/prefixItems/{index}"),
                    base,
                    depth + 1,
                )?;
                evaluated.items.insert(index);
            }
        }
        if let Some(schema) = obj.get("items") {
            for (index, item) in array.iter().enumerate().skip(prefix_len) {
                self.eval(
                    schema,
                    item,
                    &format!("{path}/{index}"),
                    &format!("{schema_path}/items"),
                    base,
                    depth + 1,
                )?;
                evaluated.items.insert(index);
            }
        }
        if let Some(schema) = obj.get("contains") {
            let mut count = 0usize;
            for (index, item) in array.iter().enumerate() {
                if self
                    .eval(
                        schema,
                        item,
                        &format!("{path}/{index}"),
                        &format!("{schema_path}/contains"),
                        base,
                        depth + 1,
                    )
                    .is_ok()
                {
                    count += 1;
                    evaluated.items.insert(index);
                }
            }
            let count = Value::Number(crate::value::Number::from_yaml(count.to_string()));
            if obj
                .get("minContains")
                .map_or(!count_at_least(&count, &Value::Number(1.into())), |min| {
                    !count_at_least(&count, min)
                })
                || obj
                    .get("maxContains")
                    .is_some_and(|max| !count_at_most(&count, max))
            {
                return Err(self.fail(
                    path,
                    &format!("{schema_path}/contains"),
                    "contains count constraint failed",
                ));
            }
        }
        if let Some(schema) = obj.get("unevaluatedItems") {
            for (index, item) in array.iter().enumerate() {
                if !evaluated.items.contains(&index) {
                    self.eval(
                        schema,
                        item,
                        &format!("{path}/{index}"),
                        &format!("{schema_path}/unevaluatedItems"),
                        base,
                        depth + 1,
                    )?;
                    evaluated.items.insert(index);
                }
            }
        }
        Ok(())
    }

    fn eval_object(
        &mut self,
        obj: &Map,
        object: &Map,
        path: &str,
        schema_path: &str,
        base: Option<&Url>,
        depth: usize,
        evaluated: &mut Evaluated,
    ) -> Result<(), Error> {
        let len = Value::Number(crate::value::Number::from_yaml(object.len().to_string()));
        for (keyword, valid) in [
            (
                "minProperties",
                obj.get("minProperties")
                    .is_none_or(|n| count_at_least(&len, n)),
            ),
            (
                "maxProperties",
                obj.get("maxProperties")
                    .is_none_or(|n| count_at_most(&len, n)),
            ),
        ] {
            if !valid {
                return Err(self.fail(
                    path,
                    &format!("{schema_path}/{keyword}"),
                    format!("{keyword} constraint failed"),
                ));
            }
        }
        if let Some(required) = obj.get("required").and_then(Value::as_array) {
            for name in required.iter().filter_map(Value::as_str) {
                if !object.contains_key(name) {
                    return Err(self.fail(
                        path,
                        &format!("{schema_path}/required"),
                        format!("missing required property {name:?}"),
                    ));
                }
            }
        }
        if let Some(names) = obj.get("propertyNames") {
            for name in object.keys() {
                self.eval(
                    names,
                    &Value::String(name.clone()),
                    path,
                    &format!("{schema_path}/propertyNames"),
                    base,
                    depth + 1,
                )?;
            }
        }
        let mut locally_matched = HashSet::new();
        for (name, value) in object {
            let child_path = format!("{path}/{}", escape(name));
            if let Some(schema) = obj
                .get("properties")
                .and_then(Value::as_object)
                .and_then(|properties| properties.get(name))
            {
                self.eval(
                    schema,
                    value,
                    &child_path,
                    &format!("{schema_path}/properties/{}", escape(name)),
                    base,
                    depth + 1,
                )?;
                evaluated.properties.insert(name.clone());
                locally_matched.insert(name.clone());
            }
            if let Some(patterns) = obj.get("patternProperties").and_then(Value::as_object) {
                for (pattern, schema) in patterns {
                    if Regex::new(pattern).map_err(Error::source)?.is_match(name) {
                        self.eval(
                            schema,
                            value,
                            &child_path,
                            &format!("{schema_path}/patternProperties/{}", escape(pattern)),
                            base,
                            depth + 1,
                        )?;
                        evaluated.properties.insert(name.clone());
                        locally_matched.insert(name.clone());
                    }
                }
            }
        }
        if let Some(dependencies) = obj.get("dependentRequired").and_then(Value::as_object) {
            for (name, required) in dependencies {
                if object.contains_key(name) {
                    for dependency in required
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                    {
                        if !object.contains_key(dependency) {
                            return Err(self.fail(
                                path,
                                &format!("{schema_path}/dependentRequired/{}", escape(name)),
                                format!("missing dependent property {dependency:?}"),
                            ));
                        }
                    }
                }
            }
        }
        if let Some(dependencies) = obj.get("dependentSchemas").and_then(Value::as_object) {
            for (name, schema) in dependencies {
                if object.contains_key(name) {
                    evaluated.merge(self.eval(
                        schema,
                        &Value::Object(object.clone()),
                        path,
                        &format!("{schema_path}/dependentSchemas/{}", escape(name)),
                        base,
                        depth + 1,
                    )?);
                }
            }
        }
        if let Some(schema) = obj.get("additionalProperties") {
            for (name, value) in object {
                if !locally_matched.contains(name) {
                    self.eval(
                        schema,
                        value,
                        &format!("{path}/{}", escape(name)),
                        &format!("{schema_path}/additionalProperties"),
                        base,
                        depth + 1,
                    )?;
                    evaluated.properties.insert(name.clone());
                }
            }
        }
        if let Some(schema) = obj.get("unevaluatedProperties") {
            for (name, value) in object {
                if !evaluated.properties.contains(name) {
                    self.eval(
                        schema,
                        value,
                        &format!("{path}/{}", escape(name)),
                        &format!("{schema_path}/unevaluatedProperties"),
                        base,
                        depth + 1,
                    )?;
                    evaluated.properties.insert(name.clone());
                }
            }
        }
        Ok(())
    }
}

fn equal(left: &Value, right: &Value) -> bool {
    if let Some(equal) = crate::numeric::equal(left, right) {
        return equal;
    }
    match (left, right) {
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| equal(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, a)| b.get(key).is_some_and(|b| equal(a, b)))
        }
        _ => left == right,
    }
}

fn count_at_least(count: &Value, bound: &Value) -> bool {
    crate::numeric::compare(count, bound).is_some_and(|order| order.is_ge())
}

fn count_at_most(count: &Value, bound: &Value) -> bool {
    crate::numeric::compare(count, bound).is_some_and(|order| order.is_le())
}

fn format_valid(format: &str, text: &str) -> bool {
    match format {
        "date-time" => date_time_valid(text),
        "date" => {
            Regex::new(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}$")
                .unwrap()
                .is_match(text)
                && jiff::civil::Date::strptime("%Y-%m-%d", text).is_ok()
        }
        "time" => time_valid(text),
        "duration" => duration_valid(text),
        "email" => email_valid(text, false),
        "idn-email" => email_valid(text, true),
        "hostname" => hostname_valid(text) && text.is_ascii() && idn_hostname_valid(text),
        "idn-hostname" => idn_hostname_valid(text),
        "ipv4" => text.parse::<std::net::Ipv4Addr>().is_ok(),
        "ipv6" => text.parse::<std::net::Ipv6Addr>().is_ok(),
        "uri" => UriStr::new(text).is_ok(),
        "iri" => IriStr::new(text).is_ok(),
        "uri-reference" => UriReferenceStr::new(text).is_ok(),
        "iri-reference" => IriReferenceStr::new(text).is_ok(),
        "uuid" => Regex::new(r"(?i)^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")
            .unwrap()
            .is_match(text),
        "uri-template" => uri_template_valid(text),
        "json-pointer" => pointer_valid(text),
        "relative-json-pointer" => {
            let digits = text.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 || digits > 1 && text.starts_with('0') {
                return false;
            }
            let mut rest = &text[digits..];
            if rest.starts_with(['+', '-']) {
                rest = &rest[1..];
                let adjustment = rest.bytes().take_while(u8::is_ascii_digit).count();
                if adjustment == 0 || adjustment > 1 && rest.starts_with('0') {
                    return false;
                }
                rest = &rest[adjustment..];
                pointer_valid(rest)
            } else {
                rest == "#" || pointer_valid(rest)
            }
        }
        "regex" => Regex::new(text).is_ok(),
        _ => false,
    }
}

fn pointer_valid(text: &str) -> bool {
    (text.is_empty() || text.starts_with('/'))
        && !text
            .as_bytes()
            .windows(2)
            .any(|pair| pair[0] == b'~' && pair[1] != b'0' && pair[1] != b'1')
        && !text.ends_with('~')
}

fn uri_template_valid(text: &str) -> bool {
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(expression) = rest.strip_prefix('{') {
            let Some(end) = expression.find('}') else {
                return false;
            };
            let mut body = &expression[..end];
            if let Some(first) = body.chars().next()
                && "+#./;?&".contains(first)
            {
                body = &body[first.len_utf8()..];
            }
            if body.is_empty() || !body.split(',').all(uri_varspec_valid) {
                return false;
            }
            rest = &expression[end + 1..];
        } else {
            let c = rest.chars().next().unwrap();
            if c == '}' || c.is_control() || c.is_whitespace() {
                return false;
            }
            if c == '%' {
                let bytes = rest.as_bytes();
                if bytes.len() < 3 || !bytes[1].is_ascii_hexdigit() || !bytes[2].is_ascii_hexdigit()
                {
                    return false;
                }
                rest = &rest[3..];
            } else {
                rest = &rest[c.len_utf8()..];
            }
        }
    }
    true
}

fn uri_varspec_valid(spec: &str) -> bool {
    if spec.is_empty() {
        return false;
    }
    let (name, modifier) = if let Some(name) = spec.strip_suffix('*') {
        (name, "*")
    } else if let Some((name, modifier)) = spec.split_once(':') {
        (name, modifier)
    } else {
        (spec, "")
    };
    if modifier != "*"
        && !modifier.is_empty()
        && (modifier.len() > 4
            || modifier.starts_with('0')
            || !modifier.bytes().all(|b| b.is_ascii_digit()))
    {
        return false;
    }
    if name.is_empty() {
        return false;
    }
    name.split('.').all(|segment| {
        if segment.is_empty() {
            return false;
        }
        let mut bytes = segment.as_bytes();
        while !bytes.is_empty() {
            if bytes[0] == b'%' {
                if bytes.len() < 3 || !bytes[1].is_ascii_hexdigit() || !bytes[2].is_ascii_hexdigit()
                {
                    return false;
                }
                bytes = &bytes[3..];
            } else if bytes[0].is_ascii_alphanumeric() || bytes[0] == b'_' {
                bytes = &bytes[1..];
            } else {
                return false;
            }
        }
        true
    })
}

fn hostname_valid(text: &str) -> bool {
    !text.is_empty()
        && !text.ends_with('.')
        && text.len() <= 253
        && text.trim_end_matches('.').split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

fn email_valid(text: &str, international: bool) -> bool {
    let Some((local, domain)) = text.rsplit_once('@') else {
        return false;
    };
    if local.is_empty() || local.len() > 64 || domain.ends_with('.') {
        return false;
    }
    let local_valid = if local.starts_with('"') {
        if !local.ends_with('"') || local.len() < 2 {
            return false;
        }
        let inner = &local[1..local.len() - 1];
        let mut escaped = false;
        let mut valid = true;
        for c in inner.chars() {
            if escaped {
                valid &= c.is_ascii() && !c.is_ascii_control();
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else {
                valid &= (c.is_ascii() && (c == ' ' || c.is_ascii_graphic()) && c != '"')
                    || international && !c.is_ascii();
            }
        }
        valid && !escaped
    } else {
        !local.starts_with('.')
            && !local.ends_with('.')
            && local.split('.').all(|part| {
                !part.is_empty()
                    && part.chars().all(|c| {
                        c.is_ascii_alphanumeric()
                            || "!#$%&'*+/=?^_`{|}~-".contains(c)
                            || international && !c.is_ascii()
                    })
            })
    };
    if !local_valid {
        return false;
    }
    if let Some(literal) = domain.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        return literal.parse::<std::net::Ipv4Addr>().is_ok()
            || literal
                .get(..5)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("IPv6:"))
                && literal[5..].parse::<std::net::Ipv6Addr>().is_ok();
    }
    if international {
        idn_hostname_valid(domain)
    } else {
        hostname_valid(domain)
    }
}

fn idn_hostname_valid(text: &str) -> bool {
    let (unicode, decoded) = idna::domain_to_unicode(text);
    if decoded.is_err() || !idn_context_valid(&unicode) {
        return false;
    }
    idna::domain_to_ascii_strict(text).is_ok_and(|ascii| {
        hostname_valid(ascii.trim_end_matches('.'))
            && idna::domain_to_ascii_strict(&unicode).is_ok_and(|reencoded| reencoded == ascii)
    })
}

fn idn_context_valid(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    if chars
        .iter()
        .any(|c| c.is_whitespace() || matches!(*c as u32, 0x00a1 | 0x0640 | 0x07fa))
    {
        return false;
    }
    for label in text.split('.') {
        let arabic_indic = label.chars().any(|c| matches!(c as u32, 0x0660..=0x0669));
        let extended_arabic_indic = label.chars().any(|c| matches!(c as u32, 0x06f0..=0x06f9));
        if arabic_indic && extended_arabic_indic {
            return false;
        }
        if label.contains('\u{30fb}')
            && !label
                .chars()
                .any(|c| c != '\u{30fb}' && matches!(c as u32, 0x3040..=0x30ff | 0x3400..=0x9fff))
        {
            return false;
        }
    }
    for (index, c) in chars.iter().enumerate() {
        match *c as u32 {
            0x302e => return false,
            0x00b7
                if index == 0
                    || index + 1 == chars.len()
                    || chars[index - 1] != 'l'
                    || chars[index + 1] != 'l' =>
            {
                return false;
            }
            0x0375
                if index + 1 == chars.len()
                    || !matches!(chars[index + 1] as u32, 0x0370..=0x03ff) =>
            {
                return false;
            }
            0x05f3 | 0x05f4
                if index == 0 || !matches!(chars[index - 1] as u32, 0x0590..=0x05ff) =>
            {
                return false;
            }
            _ => {}
        }
    }
    true
}

fn duration_valid(text: &str) -> bool {
    const TIME: &str = r"(?:[0-9]+H(?:[0-9]+M(?:[0-9]+S)?)?|[0-9]+M(?:[0-9]+S)?|[0-9]+S)";
    let pattern = format!(
        r"^(?:P[0-9]+W|P(?:[0-9]+Y(?:[0-9]+M(?:[0-9]+D)?)?|[0-9]+M(?:[0-9]+D)?|[0-9]+D)(?:T{TIME})?|PT{TIME})$"
    );
    Regex::new(&pattern).unwrap().is_match(text)
}

fn time_valid(text: &str) -> bool {
    let pattern = r"(?i)^[0-9]{2}:[0-9]{2}:[0-9]{2}(?:\.[0-9]+)?(?:Z|[+-][0-9]{2}:[0-9]{2})$";
    Regex::new(pattern).unwrap().is_match(text) && date_time_valid(&format!("2000-01-01T{text}"))
}

fn date_time_valid(text: &str) -> bool {
    let pattern = r"(?i)^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(?:\.[0-9]+)?(?:Z|[+-][0-9]{2}:[0-9]{2})$";
    if !Regex::new(pattern).unwrap().is_match(text) {
        return false;
    }
    let offset = &text[text.len() - 6..];
    if (offset.starts_with('+') || offset.starts_with('-'))
        && (offset[1..3].parse::<u8>().unwrap_or(24) > 23
            || offset[4..6].parse::<u8>().unwrap_or(60) > 59)
    {
        return false;
    }
    let mut normalized = text.to_owned();
    if let Some(dot) = text.find('.') {
        let digits = text[dot + 1..]
            .bytes()
            .take_while(u8::is_ascii_digit)
            .count();
        if digits > 9 {
            normalized.replace_range(dot + 10..dot + 1 + digits, "");
        }
    }
    let Ok(date_time) = normalized.parse::<jiff::Timestamp>() else {
        return false;
    };
    if &text[17..19] == "60" {
        let utc = date_time.to_zoned(jiff::tz::TimeZone::UTC);
        utc.hour() == 23 && utc.minute() == 59
    } else {
        true
    }
}

fn find_anchor<'a>(value: &'a Value, anchor: &str) -> Option<&'a Value> {
    find_anchor_inner(value, anchor, false, true)
}

fn find_dynamic_anchor<'a>(value: &'a Value, anchor: &str) -> Option<&'a Value> {
    find_anchor_inner(value, anchor, true, true)
}

fn find_anchor_inner<'a>(
    value: &'a Value,
    anchor: &str,
    dynamic_only: bool,
    root: bool,
) -> Option<&'a Value> {
    match value {
        Value::Object(object) => {
            if !root && object.contains_key("$id") {
                return None;
            }
            if object.get("$dynamicAnchor").and_then(Value::as_str) == Some(anchor)
                || !dynamic_only && object.get("$anchor").and_then(Value::as_str) == Some(anchor)
            {
                return Some(value);
            }
            schema_children(object)
                .into_iter()
                .find_map(|value| find_anchor_inner(value, anchor, dynamic_only, false))
        }
        Value::Array(items) => items
            .iter()
            .find_map(|value| find_anchor_inner(value, anchor, dynamic_only, false)),
        _ => None,
    }
}

fn index_resources(value: &Value, inherited_base: &Url, resources: &mut HashMap<String, Value>) {
    match value {
        Value::Object(object) => {
            let owned_base = object
                .get("$id")
                .and_then(Value::as_str)
                .and_then(|id| inherited_base.join(id).ok());
            let base = owned_base.as_ref().unwrap_or(inherited_base);
            if let Some(base) = owned_base.as_ref() {
                let mut resource = base.clone();
                resource.set_fragment(None);
                resources.insert(resource.to_string(), value.clone());
            }
            for child in schema_children(object) {
                index_resources(child, base, resources);
            }
        }
        Value::Array(items) => {
            for item in items {
                index_resources(item, inherited_base, resources);
            }
        }
        _ => {}
    }
}

fn schema_children(object: &Map) -> Vec<&Value> {
    let mut children = Vec::new();
    for keyword in [
        "$defs",
        "properties",
        "patternProperties",
        "dependentSchemas",
    ] {
        if let Some(entries) = object.get(keyword).and_then(Value::as_object) {
            children.extend(entries.values());
        }
    }
    for keyword in [
        "items",
        "additionalItems",
        "additionalProperties",
        "unevaluatedItems",
        "unevaluatedProperties",
        "contains",
        "propertyNames",
        "not",
        "if",
        "then",
        "else",
        "contentSchema",
    ] {
        if let Some(child) = object.get(keyword) {
            children.push(child);
        }
    }
    for keyword in ["allOf", "anyOf", "oneOf", "prefixItems"] {
        if let Some(items) = object.get(keyword).and_then(Value::as_array) {
            children.extend(items);
        }
    }
    children
}

fn percent_decode(input: &str) -> Result<String, Error> {
    let mut out = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err(Error::new("invalid reference fragment encoding"));
            }
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).map_err(Error::source)?;
            out.push(u8::from_str_radix(hex, 16).map_err(Error::source)?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(Error::source)
}

#[cfg(test)]
mod tests {
    use super::*;
    use yaml_rt_core::YamlDoc;
    #[test]
    fn validates_nested_properties_and_required() {
        let schema = Schema::parse(
            r#"{"type":"object","required":["name"],"properties":{"name":{"type":"string"}}}"#,
            None,
        )
        .unwrap();
        assert!(
            schema
                .validate(&YamlDoc::parse("name: Ada\n").unwrap(), 0)
                .is_ok()
        );
        assert!(
            schema
                .validate(&YamlDoc::parse("name: 42\n").unwrap(), 0)
                .is_err()
        );
        assert!(
            schema
                .validate(&YamlDoc::parse("other: Ada\n").unwrap(), 0)
                .is_err()
        );
    }

    #[test]
    fn format_annotation_and_assertion_are_distinct() {
        let schema = Schema::parse(r#"{"format":"ipv4"}"#, None).unwrap();
        let bad = Value::String("not an address".into());
        assert!(schema.validate_json(&bad).is_ok());
        assert!(
            schema
                .with_format_assertions(true)
                .validate_json(&bad)
                .is_err()
        );
        let local_vocabulary = Schema::parse(r#"{"$vocabulary":{"https://json-schema.org/draft/2020-12/vocab/format-assertion":true},"format":"ipv4"}"#, None).unwrap();
        assert!(local_vocabulary.validate_json(&bad).is_ok());
        assert!(
            Schema::parse(
                r#"{"$vocabulary":{"https://example.com/unknown":true}}"#,
                None
            )
            .is_ok()
        );
    }

    #[test]
    fn large_count_constraints_are_exact() {
        for (schema, instance, valid) in [
            (r#"{"minItems":100000000000000000000}"#, "[]", false),
            (r#"{"minLength":100000000000000000000}"#, "\"\"", false),
            (r#"{"maxItems":100000000000000000000}"#, "[]", true),
            (r#"{"minProperties":100000000000000000000}"#, "{}", false),
            (r#"{"maxProperties":100000000000000000000}"#, "{}", true),
            (
                r#"{"contains":true,"minContains":100000000000000000000}"#,
                "[]",
                false,
            ),
            (
                r#"{"contains":true,"maxContains":100000000000000000000}"#,
                "[1]",
                true,
            ),
        ] {
            let schema = Schema::parse(schema, None).unwrap();
            assert_eq!(
                schema
                    .validate_json(&Value::parse(instance).unwrap())
                    .is_ok(),
                valid
            );
        }
    }

    #[test]
    fn reference_rejects_invalid_array_indices() {
        for index in ["01", "+1", "-1", ""] {
            let schema = Schema::parse(
                &format!(
                    r##"{{"$defs":{{"x":{{"prefixItems":[false,true]}}}},"$ref":"#/$defs/x/prefixItems/{index}"}}"##
                ),
                None,
            )
            .unwrap();
            let error = schema.validate_json(&Value::Null).unwrap_err();
            assert!(
                error.to_string().contains("unresolved reference"),
                "{index}: {error}"
            );
        }
    }

    #[test]
    fn relative_pointer_index_manipulation_format() {
        let schema = Schema::parse(r#"{"format":"relative-json-pointer"}"#, None)
            .unwrap()
            .with_format_assertions(true);
        for pointer in ["0+1/foo", "2-0", "0+0", "0#"] {
            assert!(
                schema.validate_json(&Value::String(pointer.into())).is_ok(),
                "{pointer}"
            );
        }
        for pointer in ["0+01/foo", "0+", "01-1/foo", "0+1#"] {
            assert!(
                schema
                    .validate_json(&Value::String(pointer.into()))
                    .is_err(),
                "{pointer}"
            );
        }
    }

    #[test]
    fn registered_resource_uses_meta_schema() {
        let mut schema = Schema::parse("true", None).unwrap();
        assert!(
            schema
                .register_resource("https://example.com/bad", r#"{"type":["string","string"]}"#)
                .is_err()
        );
    }

    #[test]
    fn file_resource_uses_meta_schema() {
        let path = std::env::temp_dir().join(format!(
            "yaml-rt-invalid-schema-{}.json",
            std::process::id()
        ));
        std::fs::write(&path, r#"{"type":["string","string"]}"#).unwrap();
        let reference = url::Url::from_file_path(&path).unwrap().to_string();
        let schema = Schema::parse(&format!(r#"{{"$ref":"{reference}"}}"#), None).unwrap();
        let result = schema.validate_json(&Value::Null);
        std::fs::remove_file(&path).unwrap();
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("invalid 2020-12 schema")
        );
    }

    #[test]
    fn invalid_schema_keyword_type_is_rejected() {
        assert!(Schema::parse(r#"{"required":"name"}"#, None).is_err());
        assert!(Schema::parse(r#"{"minimum":"zero"}"#, None).is_err());
    }
}
