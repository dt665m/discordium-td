//! Presentation dependencies resolve only against the exact published parent scope.
use super::{ClientScopes, Payload, ReplicationError, validate_scope};
use crate::types::ScopeIdentity;
use serde::{Deserialize, Serialize};

/// The game validates transforms and chooses whether an independent fallback is
/// permitted. This reference grants no entitlement to the parent or its children.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attachment<T> {
    pub parent: ScopeIdentity,
    pub revision: u64,
    pub local: T,
    pub independent: Option<T>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AttachmentResolution<'a, T, P> {
    Parent { local: &'a T, parent: &'a P },
    Independent(&'a T),
    Staged,
}

impl<T> Attachment<T> {
    pub fn resolve<'a, P: Payload>(
        &'a self,
        child: ScopeIdentity,
        scopes: &'a ClientScopes<P>,
    ) -> Result<AttachmentResolution<'a, T, P>, ReplicationError> {
        validate_scope(child, scopes.connection())?;
        validate_scope(self.parent, scopes.connection())?;
        if self.revision == 0 || self.parent.entity == child.entity {
            return Err(ReplicationError::InvalidIdentity);
        }
        if scopes
            .state(child.entity)
            .is_none_or(|state| state.scope != child)
        {
            return Err(ReplicationError::Stale);
        }
        if let Some(parent) = scopes.state(self.parent.entity)
            && parent.scope == self.parent
        {
            return Ok(AttachmentResolution::Parent {
                local: &self.local,
                parent: &parent.payload,
            });
        }
        Ok(self.independent.as_ref().map_or(
            AttachmentResolution::Staged,
            AttachmentResolution::Independent,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{replication::*, types::*};
    fn scope(index: u64, generation: u32, incarnation: u64) -> ScopeIdentity {
        ScopeIdentity {
            connection: ConnectionEpoch(1),
            entity: EntityId { index, generation },
            scope: ScopeEpoch(incarnation),
            representation: RepresentationRevision(1),
        }
    }
    fn publish(scopes: &mut ClientScopes<Vec<u8>>, scope: ScopeIdentity) {
        scopes
            .apply_full(
                FullState {
                    scope,
                    baseline_generation: BaselineGeneration(1),
                    snapshot: SnapshotId(1),
                    version: StateVersion(1),
                    end_tick: ServerTick(1),
                    payload: vec![1],
                },
                |_| true,
            )
            .unwrap();
    }
    #[test]
    fn missing_reentered_and_reused_parent_never_bind_by_index() {
        let mut scopes = ClientScopes::new(ConnectionEpoch(1), ScopeLimits::default()).unwrap();
        let child = scope(2, 1, 1);
        let parent = scope(1, 1, 1);
        publish(&mut scopes, child);
        let mut attachment = Attachment {
            parent,
            revision: 1,
            local: [0.0, 1.0, 0.0],
            independent: None,
        };
        assert_eq!(
            attachment.resolve(child, &scopes),
            Ok(AttachmentResolution::Staged)
        );
        publish(&mut scopes, parent);
        assert!(matches!(
            attachment.resolve(child, &scopes),
            Ok(AttachmentResolution::Parent { .. })
        ));
        scopes.apply_exit(ScopeExit { scope: parent }).unwrap();
        publish(&mut scopes, scope(1, 1, 2));
        assert_eq!(
            attachment.resolve(child, &scopes),
            Ok(AttachmentResolution::Staged)
        );
        attachment.parent = scope(1, 1, 2);
        assert!(matches!(
            attachment.resolve(child, &scopes),
            Ok(AttachmentResolution::Parent { .. })
        ));
        publish(&mut scopes, scope(1, 2, 1));
        assert_eq!(
            attachment.resolve(child, &scopes),
            Ok(AttachmentResolution::Staged)
        );
        attachment.independent = Some([2.0, 1.0, 0.0]);
        assert_eq!(
            attachment.resolve(child, &scopes),
            Ok(AttachmentResolution::Independent(&[2.0, 1.0, 0.0]))
        );
        scopes.apply_exit(ScopeExit { scope: child }).unwrap();
        assert_eq!(
            attachment.resolve(child, &scopes),
            Err(ReplicationError::Stale)
        );
    }
}
