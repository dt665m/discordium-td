//! Complete current map-object state carried by ordinary scoped replication.
//! No event replay or additional tombstone cache is required on region entry.
use super::ReplicationError;
use crate::{codec::BoundedVec, types::SceneRevision};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MapObjectState {
    pub slot: u32,
    pub generation: u32,
    pub present: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn current_absence_and_unknown_generations_never_restore_map_defaults() {
        let manifest = RegionManifest::<2> {
            scene: SceneRevision(1),
            region: 1,
            revision: 2,
            objects: BoundedVec::new(vec![MapObjectState {
                slot: 1,
                generation: 3,
                present: false,
            }])
            .unwrap(),
        };
        assert_eq!(manifest.presence(1, 3), MapObjectPresence::Absent);
        assert_eq!(manifest.presence(1, 2), MapObjectPresence::Unavailable);
        assert_eq!(manifest.presence(2, 3), MapObjectPresence::Unavailable);
        let mut invalid = manifest.clone();
        invalid
            .objects
            .push(MapObjectState {
                slot: 1,
                generation: 4,
                present: true,
            })
            .unwrap();
        assert_eq!(invalid.validate(), Err(ReplicationError::InvalidPayload));
        assert_eq!(invalid.presence(1, 4), MapObjectPresence::Unavailable);
        let bytes = bincode::serialize(&manifest).unwrap();
        let restored: RegionManifest<2> = bincode::deserialize(&bytes).unwrap();
        assert_eq!(restored, manifest);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegionManifest<const N: usize> {
    pub scene: SceneRevision,
    pub region: u32,
    pub revision: u64,
    pub objects: BoundedVec<MapObjectState, N>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapObjectPresence {
    Present,
    Absent,
    /// Missing region, missing slot, or a different generation is never license
    /// to instantiate a cached map default.
    Unavailable,
}

impl<const N: usize> RegionManifest<N> {
    pub fn validate(&self) -> Result<(), ReplicationError> {
        if self.scene.0 == 0 || self.region == 0 || self.revision == 0 {
            return Err(ReplicationError::InvalidIdentity);
        }
        let mut previous = 0;
        for object in self.objects.as_slice() {
            if object.slot <= previous || object.generation == 0 {
                return Err(ReplicationError::InvalidPayload);
            }
            previous = object.slot;
        }
        Ok(())
    }

    pub fn presence(&self, slot: u32, generation: u32) -> MapObjectPresence {
        if self.validate().is_err() {
            return MapObjectPresence::Unavailable;
        }
        self.objects
            .as_slice()
            .iter()
            .find(|object| object.slot == slot)
            .filter(|object| object.generation == generation)
            .map_or(MapObjectPresence::Unavailable, |object| {
                if object.present {
                    MapObjectPresence::Present
                } else {
                    MapObjectPresence::Absent
                }
            })
    }
}
