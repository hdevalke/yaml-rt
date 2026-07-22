//! Public facade for YAML round-trip parsing and typed overlays.

pub use yaml_rt_core::*;

#[cfg(feature = "derive")]
pub use yaml_rt_derive::YamlRoundTrip;

#[cfg(feature = "serde")]
pub use yaml_rt_serde::{
    Deserializer, Error, Location, Result, Serializer, from_reader, from_slice, from_str,
    to_string, to_writer,
};
