//! Event disclosure is independent of entity relevance and remains subject to
//! the current policy barrier. Anonymous delivery does not imply authorization.
use super::ReplicationError;
use crate::types::{ConnectionId, PolicyRevision};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventDisclosure {
    /// Includes the source only when its representation is independently allowed.
    SourceVisible,
    /// The game supplies an approved coarse origin and source-free payload.
    Anonymous,
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Policy {
        revision: PolicyRevision,
    }
    impl EventAudiencePolicy<u8> for Policy {
        fn revision(&self) -> PolicyRevision {
            self.revision
        }
        fn disclosure(&self, connection: ConnectionId, event: &u8) -> Option<EventDisclosure> {
            match (connection.0, event) {
                (1, 1) => Some(EventDisclosure::SourceVisible),
                (2, 1) => Some(EventDisclosure::Anonymous),
                _ => None,
            }
        }
    }
    #[test]
    fn denied_class_or_origin_never_projects_and_revocation_invalidates_approval() {
        let policy = Policy {
            revision: PolicyRevision(1),
        };
        assert!(EventAudience::authorize(ConnectionId(2), &2, &policy).is_none());
        assert!(EventAudience::authorize(ConnectionId(3), &1, &policy).is_none());
        let audience = EventAudience::authorize(ConnectionId(2), &1, &policy).unwrap();
        assert_eq!(
            audience.project(
                ConnectionId(2),
                PolicyRevision(1),
                |_| panic!("hidden source"),
                |_| Some("coarse cell")
            ),
            Ok("coarse cell")
        );
        assert_eq!(
            audience.project(ConnectionId(1), PolicyRevision(1), |_| Some(1), |_| Some(2)),
            Err(ReplicationError::Unauthorized)
        );
        assert_eq!(
            audience.project(ConnectionId(2), PolicyRevision(2), |_| Some(1), |_| Some(2)),
            Err(ReplicationError::Unauthorized)
        );
        let visible = EventAudience::authorize(ConnectionId(1), &1, &policy).unwrap();
        assert_eq!(
            visible.project(
                ConnectionId(1),
                PolicyRevision(1),
                |_| None::<u8>,
                |_| Some(2)
            ),
            Err(ReplicationError::Unauthorized)
        );
    }
}

pub trait EventAudiencePolicy<E> {
    fn revision(&self) -> PolicyRevision;
    /// Unknown event classes and prohibited origins must return None, including
    /// when the event's source happens to be spatially relevant.
    fn disclosure(&self, connection: ConnectionId, event: &E) -> Option<EventDisclosure>;
}

/// Non-serializable authority approval. Recheck the barrier immediately before
/// encoding, then offer the positive projection through ordinary ServerScopes.
#[derive(Debug, Clone, Copy)]
pub struct EventAudience<'a, E> {
    event: &'a E,
    connection: ConnectionId,
    policy: PolicyRevision,
    disclosure: EventDisclosure,
}
impl<'a, E> EventAudience<'a, E> {
    pub fn authorize(
        connection: ConnectionId,
        event: &'a E,
        policy: &impl EventAudiencePolicy<E>,
    ) -> Option<Self> {
        let revision = policy.revision();
        (connection.0 != 0 && revision.0 != 0).then_some(())?;
        let disclosure = policy.disclosure(connection, event)?;
        if policy.revision() != revision {
            return None;
        }
        Some(Self {
            event,
            connection,
            policy: revision,
            disclosure,
        })
    }

    pub fn project<P>(
        self,
        connection: ConnectionId,
        current_policy: PolicyRevision,
        visible: impl FnOnce(&E) -> Option<P>,
        anonymous: impl FnOnce(&E) -> Option<P>,
    ) -> Result<P, ReplicationError> {
        if self.connection != connection || self.policy != current_policy {
            return Err(ReplicationError::Unauthorized);
        }
        match self.disclosure {
            EventDisclosure::SourceVisible => visible(self.event),
            EventDisclosure::Anonymous => anonymous(self.event),
        }
        .ok_or(ReplicationError::Unauthorized)
    }
}
