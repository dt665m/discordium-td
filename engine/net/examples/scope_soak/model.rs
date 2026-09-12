//! Small deterministic fixture using the production prediction manager.
use engine_net::{commands::OwnerStream, prediction::*, replication::*, types::*};
use std::collections::{BTreeMap, BTreeSet};

pub struct Model {
    pub dependency: EntityId,
}
#[derive(Clone, PartialEq)]
pub struct Input {
    pub owner: OwnerStream,
    pub tick: u64,
}
impl Payload for Input {
    fn retained_bytes(&self) -> usize {
        0
    }
}
impl PredictionModel for Model {
    type Wire = Vec<u8>;
    type State = Vec<u8>;
    type Command = Input;
    type Dependency = Vec<u8>;
    fn decode_group(
        &self,
        proof: &PublishedGroup<'_, Vec<u8>>,
        binding: PredictionBinding,
    ) -> Result<DecodedPrediction<Vec<u8>, Vec<u8>>, PredictionError> {
        let owner = proof
            .states()
            .find(|s| s.scope.entity == binding.owner.owner)
            .ok_or(PredictionError::InvalidState)?;
        let dependency = proof
            .states()
            .find(|s| s.scope.entity == self.dependency)
            .ok_or(PredictionError::InvalidDependency)?;
        Ok(DecodedPrediction {
            state: owner.payload.clone(),
            dependencies: DependencyFrame {
                records: BTreeMap::from([(
                    self.dependency,
                    DependencyRecord {
                        scope: dependency.scope,
                        scene_revision: binding.scene_revision,
                        version: dependency.version,
                        value: DependencyValue::Known(dependency.payload.clone()),
                    },
                )]),
            },
        })
    }
    fn valid_state(&self, state: &Vec<u8>) -> bool {
        state.len() == 64
    }
    fn command_identity(&self, input: &Input) -> (OwnerStream, CommandSeq, TargetTick) {
        (input.owner, CommandSeq(input.tick), TargetTick(input.tick))
    }
    fn valid_command(&self, input: &Input) -> bool {
        input.tick > 0
    }
    fn required_dependencies(&self, _: &Vec<u8>, _: Option<&Input>) -> Vec<EntityId> {
        vec![self.dependency]
    }
    fn step(
        &self,
        state: &mut Vec<u8>,
        tick: ServerTick,
        _: &Input,
        dependencies: &DependencyFrame<Vec<u8>>,
    ) -> Result<BTreeSet<ActionKey>, PredictionError> {
        if !matches!(
            dependencies.value(self.dependency),
            Some(DependencyValue::Known(_))
        ) {
            return Err(PredictionError::InvalidDependency);
        }
        state[..8].copy_from_slice(&tick.0.to_le_bytes());
        Ok(BTreeSet::new())
    }
}
