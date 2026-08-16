//! YAML 1.2.2 parsing, source-preserving editing, typed overlays, and optional
//! Serde conversion.
//!
//! `YamlDoc` keeps the original source as its source of truth. Unedited YAML is
//! returned byte-for-byte, while editor operations queue localized changes and
//! [`YamlPatch`] applies multi-operation YAML or JSON patches transactionally.
//!
//! ```
//! use yaml_rt::{YamlDoc, YamlError};
//!
//! # fn main() -> Result<(), YamlError> {
//! let mut doc = YamlDoc::parse("port: 8080 # selected port\n")?;
//! doc.set_scalar(&["port"], "9090")?;
//! assert_eq!(doc.to_string(), "port: 9090 # selected port\n");
//! # Ok(())
//! # }
//! ```
//!
//! The default `derive` feature re-exports [`YamlRt`] for named mapping
//! structs, transparent newtypes, and locally tagged enums. The optional
//! `serde` feature re-exports serialization and deserialization APIs for
//! conversions where presentation preservation is not required, including a
//! generic `Value` model.

pub use yaml_rt_core::*;

#[cfg(feature = "derive")]
pub use yaml_rt_derive::YamlRt;

#[cfg(feature = "serde")]
pub use yaml_rt_serde::{
    Deserializer, Error, Index, Location, Mapping, Number, Result, Sequence, Serializer, Value,
    from_reader, from_slice, from_str, from_value, to_string, to_value, to_writer,
};
