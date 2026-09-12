//! Generated positive field codecs; authoritative validation follows staged decode.
use super::*;
impl DreamGlobalView {
    pub fn encode(&self) -> Result<Vec<u8>, ReplicationError> {
        schema::encode_global(self)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, ReplicationError> {
        schema::decode_global(bytes)
    }
}
impl PublicReplica {
    pub fn validate(&self) -> Result<(), ReplicationError> {
        super::presentation::validate_replica(self)
    }
    pub fn encode(&self) -> Result<Vec<u8>, ReplicationError> {
        schema::encode_public(self)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, ReplicationError> {
        schema::decode_public(bytes)
    }
    /// Live representation after the session has negotiated the exact protocol
    /// and `schema_identity`. Canonical offline encoding remains `encode`.
    pub fn encode_compact(&self) -> Result<Vec<u8>, ReplicationError> {
        schema::encode_public_compact(self)
    }
    /// Decode only after the enclosing session validates its exact protocol and
    /// schema identity. This body does not carry a redundant schema fingerprint.
    pub fn decode_compact(bytes: &[u8]) -> Result<Self, ReplicationError> {
        schema::decode_public_compact(bytes)
    }
}
