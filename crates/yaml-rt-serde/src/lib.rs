//! Serde serialization and deserialization for RTY's YAML 1.2.2 model.
//!
//! This crate converts typed values. It intentionally does not preserve YAML
//! presentation details; use `yaml-rt`'s `YamlRoundTrip` overlay for lossless
//! document editing.

mod de;
mod error;
mod ser;

pub use de::{Deserializer, from_reader, from_slice, from_str};
pub use error::{Error, Location, Result};
pub use ser::{Serializer, to_string, to_writer};
