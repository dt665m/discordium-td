//! Dreamwake's independent public-effect audience rules.
use super::ApprovedSources;
use engine_core::{GraphicsInstance, GraphicsKind};
use engine_net::{replication::audience::*, types::*};

pub(super) struct DreamEventAudience<'a> {
    pub sources: &'a ApprovedSources,
    pub policy: PolicyRevision,
}
impl EventAudiencePolicy<GraphicsInstance> for DreamEventAudience<'_> {
    fn revision(&self) -> PolicyRevision {
        self.policy
    }
    fn disclosure(&self, _: ConnectionId, effect: &GraphicsInstance) -> Option<EventDisclosure> {
        // This is an independent event-origin restriction. Anonymous identity
        // cannot bypass it, and an approved source does not authorize an origin.
        if effect
            .pos
            .iter()
            .any(|v| !v.is_finite() || v.abs() > crate::ARENA_RADIUS)
        {
            return None;
        }
        match effect.kind {
            GraphicsKind::RadialPulse => Some(if self.sources.allows(effect.id.owner) {
                EventDisclosure::SourceVisible
            } else {
                EventDisclosure::Anonymous
            }),
            GraphicsKind::Beam { .. } | GraphicsKind::Orb { .. }
                if self.sources.allows(effect.id.owner) =>
            {
                Some(EventDisclosure::SourceVisible)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn anonymous_pulse_is_separately_authorized_and_prohibited_origin_stays_denied() {
        let sources = ApprovedSources::new([]).unwrap();
        let policy = DreamEventAudience {
            sources: &sources,
            policy: PolicyRevision(1),
        };
        let mut effect = GraphicsInstance {
            id: engine_core::GraphicsId {
                scope: None,
                match_epoch: 1,
                owner: 900_001,
                action_seq: 800_001,
                slot: 7,
            },
            kind: GraphicsKind::RadialPulse,
            pos: [2.25, -5.5],
            radius: 2.0,
            age_ticks: 0,
            duration_ticks: 20,
        };
        assert_eq!(
            policy.disclosure(ConnectionId(1), &effect),
            Some(EventDisclosure::Anonymous)
        );
        effect.kind = GraphicsKind::Beam {
            direction: [1.0, 0.0, 0.0],
            elevation: 0.9,
            length: 10.0,
        };
        assert_eq!(policy.disclosure(ConnectionId(1), &effect), None);
        effect.kind = GraphicsKind::RadialPulse;
        effect.pos = [crate::ARENA_RADIUS + 0.01, 0.0];
        assert_eq!(policy.disclosure(ConnectionId(1), &effect), None);
        let visible = ApprovedSources::new([effect.id.owner]).unwrap();
        let policy = DreamEventAudience {
            sources: &visible,
            policy: PolicyRevision(1),
        };
        assert_eq!(policy.disclosure(ConnectionId(1), &effect), None);
    }
}
