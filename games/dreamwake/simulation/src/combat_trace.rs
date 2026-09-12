//! Opt-in authority diagnostics. These records are never checkpointed or sent
//! through peer replication; the authority adapter owns retention and export.
use crate::combat::{CombatReason, CombatVerdict, RayActionKey, ValidatedRay};
use bevy::prelude::*;
use serde::Serialize;
pub const MAX_COMBAT_TRACE_RECORDS: usize = 128;
pub const MAX_COMBAT_TRACE_CREDITS: usize = 64;
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CombatTracePose {
    pub id: u64,
    pub generation: u32,
    pub segment: u64,
    pub pose_revision: u64,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub shape: engine_core::CollisionShape,
    pub faction: u8,
    pub invulnerable: bool,
    pub shield_cutoff: u64,
    pub shield: f32,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CombatTraceShieldCredit {
    pub id: u64,
    pub activated_tick: u32,
    pub expires_tick: u32,
    pub granted: f32,
    pub spent_before: f32,
    pub spent_after: f32,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CombatTraceRecord {
    pub key: RayActionKey,
    /// Exact accepted aim sample; recurring beams keep their Begin action key separately.
    pub sample_key: RayActionKey,
    pub execution_server_tick: u64,
    pub execution_gameplay_tick: u32,
    /// Exact validated view time. Requested timing belongs to adapter metadata.
    pub query_server_tick: u64,
    pub query_gameplay_tick: u32,
    pub command_fraction: Option<u16>,
    pub query_fraction: u16,
    pub muzzle: Option<[f32; 3]>,
    pub aim: [f32; 2],
    pub selected: Option<CombatTracePose>,
    pub shield_credits: Vec<CombatTraceShieldCredit>,
    pub verdict: CombatVerdict,
}
impl CombatTraceRecord {
    /// Conservative retained/export payload allowance, including bounded credits.
    /// Fixed fields fit 4KiB even with maximum decimal identities; each credit's
    /// finite scalar JSON and allocation fit its 256-byte allowance.
    pub fn retained_bytes(&self) -> usize {
        4096 + self.shield_credits.len() * 256
    }
}
pub(crate) struct PendingTrace {
    pub record: CombatTraceRecord,
    pub receipt: usize,
}
#[derive(Resource, Default)]
pub(crate) struct CombatTraceState {
    pub enabled: bool,
    pub pending: Vec<PendingTrace>,
}
impl CombatTraceState {
    pub fn begin(
        &mut self,
        ray: ValidatedRay,
        tick: u32,
        query: u32,
        receipt: usize,
    ) -> Option<usize> {
        if !self.enabled || self.pending.len() >= MAX_COMBAT_TRACE_RECORDS {
            return None;
        }
        let index = self.pending.len();
        self.pending.push(PendingTrace {
            receipt,
            record: CombatTraceRecord {
                key: ray.key,
                sample_key: ray.key,
                execution_server_tick: ray.execution_server_tick,
                execution_gameplay_tick: tick,
                query_server_tick: ray.query_server_tick,
                query_gameplay_tick: query,
                command_fraction: ray.command_fraction,
                query_fraction: ray.query_fraction,
                muzzle: None,
                aim: ray.aim,
                selected: None,
                shield_credits: Vec::new(),
                verdict: CombatVerdict {
                    key: ray.key,
                    execution_server_tick: ray.execution_server_tick,
                    execution_gameplay_tick: tick,
                    query_server_tick: ray.query_server_tick,
                    query_gameplay_tick: query,
                    reason: CombatReason::InvalidAction,
                    target: None,
                    hit_region: 0,
                    damage: 0.0,
                    transaction: None,
                },
            },
        });
        Some(index)
    }
    pub fn select(&mut self, index: Option<usize>, pose: &engine_core::CombatPose) {
        let Some(index) = index else {
            return;
        };
        self.pending[index].record.selected = Some(CombatTracePose {
            id: pose.entity.index,
            generation: pose.entity.generation,
            segment: pose.segment,
            pose_revision: pose.pose_revision,
            position: pose.position.to_array(),
            rotation: pose.rotation.to_array(),
            shape: pose.shape,
            faction: (pose.metadata.0[0] & 255) as u8,
            invulnerable: pose.metadata.0[0] & (1 << 10) != 0,
            shield_cutoff: pose.metadata.0[1],
            shield: f32::from_bits(pose.metadata.0[2] as u32),
        });
    }
}
impl crate::DreamSimulation {
    pub fn set_combat_trace_enabled(&mut self, enabled: bool) {
        let mut trace = self.world.resource_mut::<CombatTraceState>();
        trace.enabled = enabled;
        trace.pending.clear();
    }
    pub fn take_combat_traces(&mut self) -> Vec<CombatTraceRecord> {
        std::mem::take(&mut self.world.resource_mut::<CombatTraceState>().pending)
            .into_iter()
            .map(|v| v.record)
            .collect()
    }
}
pub(crate) fn finalize_combat_traces(
    mut trace: ResMut<CombatTraceState>,
    transactions: Res<crate::combat::ActionTransactions>,
) {
    for pending in &mut trace.pending {
        if let Some(verdict) = transactions
            .verdicts
            .get(pending.receipt)
            .and_then(|v| v.combat.as_ref())
        {
            pending.record.verdict = verdict.clone();
        }
    }
}
