//! Replayable domain-separated random streams for simulation entities.
//!
//! One keyed hash consumes one saved counter value. Adding an unrelated entity or
//! drawing from another domain cannot move this stream. This is deterministic
//! simulation machinery, not entropy for authentication tokens or private keys.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RandomError {
    InvalidGeneration,
    Exhausted,
}
impl std::fmt::Display for RandomError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "simulation random stream: {self:?}")
    }
}
impl std::error::Error for RandomError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RandomStream {
    key: [u8; 32],
    counter: u64,
}
/// Complete typed snapshot for field-generated codecs. Restoring this state does
/// not authenticate its key; the game still validates the entity/domain binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RandomStreamState {
    pub key: [u8; 32],
    pub counter: u64,
}
impl RandomStream {
    pub fn snapshot(&self) -> RandomStreamState {
        RandomStreamState {
            key: self.key,
            counter: self.counter,
        }
    }
    pub fn from_state(state: RandomStreamState) -> Self {
        Self {
            key: state.key,
            counter: state.counter,
        }
    }
    pub fn new(
        match_seed: u64,
        entity_index: u64,
        generation: u32,
        domain: u64,
    ) -> Result<Self, RandomError> {
        if generation == 0 {
            return Err(RandomError::InvalidGeneration);
        }
        let mut material = [0u8; 28];
        material[..8].copy_from_slice(&match_seed.to_le_bytes());
        material[8..16].copy_from_slice(&entity_index.to_le_bytes());
        material[16..20].copy_from_slice(&generation.to_le_bytes());
        material[20..].copy_from_slice(&domain.to_le_bytes());
        Ok(Self {
            key: blake3::derive_key("engine simulation entity random stream v1", &material),
            counter: 0,
        })
    }
    /// Validate a restored key against the game-owned identity/domain binding.
    /// The counter is the next draw, and remains exactly as restored.
    pub fn belongs_to(
        &self,
        match_seed: u64,
        entity_index: u64,
        generation: u32,
        domain: u64,
    ) -> bool {
        Self::new(match_seed, entity_index, generation, domain)
            .is_ok_and(|stream| stream.key == self.key)
    }
    pub fn counter(&self) -> u64 {
        self.counter
    }
    pub fn is_exhausted(&self) -> bool {
        self.counter == u64::MAX
    }
    /// Exhaustion is explicit and leaves the stream unchanged; it never wraps and
    /// repeats prior draws. The authority must apply its declared failure policy.
    pub fn next_u64(&mut self) -> Result<u64, RandomError> {
        let next = self.counter.checked_add(1).ok_or(RandomError::Exhausted)?;
        let hash = blake3::keyed_hash(&self.key, &self.counter.to_le_bytes());
        let value = u64::from_le_bytes(hash.as_bytes()[..8].try_into().expect("fixed hash width"));
        self.counter = next;
        Ok(value)
    }
    /// Exactly representable 24-bit unit interval sample in [0, 1).
    pub fn next_unit_f32(&mut self) -> Result<f32, RandomError> {
        self.next_u64()
            .map(|value| ((value >> 40) as u32 as f32) / 16_777_216.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn streams_are_entity_generation_seed_and_domain_separated() {
        let mut baseline = RandomStream::new(7, 12, 1, 42).unwrap();
        let mut replay = baseline.clone();
        let mut other = RandomStream::new(7, 13, 1, 42).unwrap();
        let mut cosmetic = RandomStream::new(7, 12, 1, 43).unwrap();
        let mut generation = RandomStream::new(7, 12, 2, 42).unwrap();
        let mut seed = RandomStream::new(8, 12, 1, 42).unwrap();
        let first = baseline.next_u64().unwrap();
        assert_ne!(first, other.next_u64().unwrap());
        assert_ne!(first, cosmetic.next_u64().unwrap());
        assert_ne!(first, generation.next_u64().unwrap());
        assert_ne!(first, seed.next_u64().unwrap());
        assert_eq!(first, replay.next_u64().unwrap());
        for _ in 0..100 {
            for _ in 0..7 {
                other.next_u64().unwrap();
                cosmetic.next_u64().unwrap();
            }
            assert_eq!(baseline.next_u64(), replay.next_u64());
        }
    }
    #[test]
    fn saved_counter_resumes_exactly_and_exhaustion_does_not_wrap() {
        let mut stream = RandomStream::new(7, 12, 1, 42).unwrap();
        for _ in 0..17 {
            stream.next_u64().unwrap();
        }
        let bytes = serde_json::to_vec(&stream).unwrap();
        let mut restored: RandomStream = serde_json::from_slice(&bytes).unwrap();
        let mut typed = RandomStream::from_state(stream.snapshot());
        assert_eq!(typed.snapshot(), stream.snapshot());
        assert!(restored.belongs_to(7, 12, 1, 42));
        assert!(!restored.belongs_to(7, 13, 1, 42));
        for _ in 0..100 {
            let expected = stream.next_unit_f32();
            assert_eq!(expected, restored.next_unit_f32());
            assert_eq!(expected, typed.next_unit_f32());
        }
        stream.counter = u64::MAX - 1;
        assert!(stream.next_u64().is_ok());
        let before = stream.clone();
        assert_eq!(stream.next_u64(), Err(RandomError::Exhausted));
        assert_eq!(stream, before);
        assert!(stream.is_exhausted());
        assert_eq!(
            RandomStream::new(0, 0, 0, 0),
            Err(RandomError::InvalidGeneration)
        );
    }
}
