//! Serde serialization and deserialization for yaml-rt's YAML 1.2.2 model.
//!
//! This crate converts typed values. It intentionally does not preserve YAML
//! presentation details; use `yaml-rt`'s `YamlRt` overlay for lossless
//! document editing.
//!
//! [`from_str`] and [`from_slice`] deserialize one YAML document. [`from_reader`]
//! accepts any byte reader. [`to_string`] and [`to_writer`] emit deterministic
//! block-style YAML.

mod de;
mod error;
mod ser;

pub use de::{Deserializer, from_reader, from_slice, from_str};
pub use error::{Error, Location, Result};
pub use ser::{Serializer, to_string, to_writer};
