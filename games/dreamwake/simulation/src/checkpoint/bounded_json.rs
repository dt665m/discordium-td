//! Allocation-free structural preflight. Serde JSON's temporary unescape buffer
//! is bounded by the input byte cap; no collection/Value allocations occur here.
use super::{CheckpointError, MAX_BYTES, MAX_COLLECTION, MAX_STRING, fault, json_error};
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use std::fmt;

// The declared 128-hero/77-enemy/45-cover profile retains 8,000 combat
// poses across 32 frames: approximately 378k nodes and 1.22MB of key/text data.
// These independent structural caps remain below the unchanged 8MiB byte bound.
const MAX_NODES: usize = 524_288;
const MAX_TEXT_BYTES: usize = 2 * 1024 * 1024;
#[derive(Clone, Copy)]
pub(crate) struct Limits {
    pub bytes: usize,
    pub collection: usize,
    pub map: usize,
    pub string: usize,
    pub nodes: usize,
    pub text: usize,
    pub depth: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            bytes: MAX_BYTES,
            collection: MAX_COLLECTION,
            map: 256,
            string: MAX_STRING,
            nodes: MAX_NODES,
            text: MAX_TEXT_BYTES,
            depth: 64,
        }
    }
}
#[derive(Default)]
struct Budget {
    nodes: usize,
    text: usize,
    limits: Limits,
}
struct Seed<'a> {
    budget: &'a mut Budget,
    depth: usize,
    allowed: bool,
}
impl<'de> DeserializeSeed<'de> for Seed<'_> {
    type Value = ();
    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        if !self.allowed {
            return Err(de::Error::custom("checkpoint collection budget exceeded"));
        }
        if self.depth > self.budget.limits.depth {
            return Err(de::Error::custom("checkpoint nesting budget exceeded"));
        }
        self.budget.nodes += 1;
        if self.budget.nodes > self.budget.limits.nodes {
            return Err(de::Error::custom(
                "checkpoint aggregate node budget exceeded",
            ));
        }
        deserializer.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for Seed<'_> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("bounded checkpoint JSON")
    }
    fn visit_bool<E: de::Error>(self, _: bool) -> Result<(), E> {
        Ok(())
    }
    fn visit_i64<E: de::Error>(self, _: i64) -> Result<(), E> {
        Ok(())
    }
    fn visit_u64<E: de::Error>(self, _: u64) -> Result<(), E> {
        Ok(())
    }
    fn visit_f64<E: de::Error>(self, value: f64) -> Result<(), E> {
        if value.is_finite() {
            Ok(())
        } else {
            Err(E::custom("nonfinite JSON number"))
        }
    }
    fn visit_unit<E: de::Error>(self) -> Result<(), E> {
        Ok(())
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<(), E> {
        if value.len() > self.budget.limits.string {
            return Err(E::custom("checkpoint string budget exceeded"));
        }
        self.budget.text += value.len();
        if self.budget.text > self.budget.limits.text {
            return Err(E::custom("checkpoint aggregate text budget exceeded"));
        }
        Ok(())
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        let mut count = 0;
        let limit = self.budget.limits.collection;
        while seq
            .next_element_seed(Seed {
                budget: self.budget,
                depth: self.depth + 1,
                allowed: count < limit,
            })?
            .is_some()
        {
            count += 1;
        }
        Ok(())
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let mut count = 0;
        let limit = self.budget.limits.map;
        while map
            .next_key_seed(Seed {
                budget: self.budget,
                depth: self.depth + 1,
                allowed: count < limit,
            })?
            .is_some()
        {
            map.next_value_seed(Seed {
                budget: self.budget,
                depth: self.depth + 1,
                allowed: true,
            })?;
            count += 1;
        }
        Ok(())
    }
}
pub(crate) fn preflight(bytes: &[u8]) -> Result<(), CheckpointError> {
    preflight_with_limits(bytes, Limits::default())
}
pub(crate) fn preflight_with_limits(bytes: &[u8], limits: Limits) -> Result<(), CheckpointError> {
    if bytes.len() > limits.bytes {
        return Err(fault("checkpoint byte budget exceeded"));
    }
    let mut parser = serde_json::Deserializer::from_slice(bytes);
    Seed {
        budget: &mut Budget {
            limits,
            ..Default::default()
        },
        depth: 0,
        allowed: true,
    }
    .deserialize(&mut parser)
    .map_err(json_error)?;
    parser.end().map_err(json_error)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_collection_and_nesting_before_value_allocation() {
        let array = format!("[{}null]", "null,".repeat(MAX_COLLECTION));
        assert!(
            preflight(array.as_bytes())
                .unwrap_err()
                .0
                .contains("collection budget")
        );
        let nested = format!("{}null{}", "[".repeat(65), "]".repeat(65));
        assert!(
            preflight(nested.as_bytes())
                .unwrap_err()
                .0
                .contains("nesting budget")
        );
    }
    #[test]
    fn aggregate_budgets_cover_many_small_containers() {
        let row = format!("[{}null]", "null,".repeat(1023));
        let value = format!("[{}]", vec![row; MAX_NODES / 1024].join(","));
        assert!(
            preflight(value.as_bytes())
                .unwrap_err()
                .0
                .contains("aggregate node")
        );
        let row = format!("\"{}\"", "x".repeat(MAX_STRING));
        let value = format!("[{}]", vec![row; MAX_TEXT_BYTES / MAX_STRING + 1].join(","));
        assert!(
            preflight(value.as_bytes())
                .unwrap_err()
                .0
                .contains("aggregate text")
        );
    }
}
