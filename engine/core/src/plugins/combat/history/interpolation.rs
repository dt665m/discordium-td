//! Immutable fractional samples; movement and gameplay clocks are never advanced.
use super::*;

fn blend(a: &CombatPose, b: &CombatPose, fraction: u16) -> Result<CombatPose, HistoryError> {
    if a.entity != b.entity {
        return Err(HistoryError::WrongLifecycle);
    }
    if a.segment != b.segment || a.shape != b.shape {
        return Err(HistoryError::Discontinuity);
    }
    let alpha = f32::from(fraction) / 65_536.0;
    Ok(CombatPose {
        position: a.position.lerp(b.position, alpha),
        rotation: a.rotation.slerp(b.rotation, alpha),
        // Discrete game metadata is the left endpoint's state until the next
        // fixed tick; interpolation does not invent partial defense activation.
        ..*a
    })
}
impl HitHistory {
    /// Sample one actor at `tick + fraction / 65536`. A nonzero fraction requires
    /// both exact adjacent ticks, scene, lifecycle, segment and unchanged shape.
    /// Zero selects the exact tick without requiring a future endpoint.
    pub fn sample_pose(
        &self,
        tick: u64,
        fraction: u16,
        scene: u64,
        entity: ColliderKey,
        segment: u64,
        budget: &mut QueryBudget,
    ) -> Result<CombatPose, HistoryError> {
        budget.charge()?;
        let left = self.frame(tick, scene)?.pose(entity, segment)?;
        if fraction == 0 {
            return Ok(*left);
        }
        budget.charge()?;
        let right = self
            .frame(
                tick.checked_add(1).ok_or(HistoryError::MissingHistory)?,
                scene,
            )?
            .pose(entity, segment)?;
        blend(left, right, fraction)
    }

    /// Build a bounded query cohort without mutating either retained endpoint.
    /// Every included actor must exist with the same eligibility at both ends;
    /// births, removals, generation/segment/shape changes reject the whole sample.
    /// The caller shares the same budget with the subsequent geometric query.
    pub fn sample_frame(
        &self,
        tick: u64,
        fraction: u16,
        scene: u64,
        budget: &mut QueryBudget,
        include: impl Fn(&CombatPose) -> bool,
    ) -> Result<HitFrame, HistoryError> {
        let left = self.frame(tick, scene)?;
        let right = if fraction == 0 {
            None
        } else {
            Some(self.frame(
                tick.checked_add(1).ok_or(HistoryError::MissingHistory)?,
                scene,
            )?)
        };
        let mut poses = Vec::with_capacity(left.poses.len());
        for a in &left.poses {
            budget.charge()?;
            if !include(a) {
                continue;
            }
            let pose = if let Some(right) = right {
                let b = right.pose(a.entity, a.segment)?;
                if !include(b) {
                    return Err(HistoryError::Discontinuity);
                }
                blend(a, b, fraction)?
            } else {
                *a
            };
            poses.push(pose);
        }
        if let Some(right) = right {
            for b in &right.poses {
                budget.charge()?;
                if include(b) {
                    let a = left.pose(b.entity, b.segment)?;
                    if !include(a) {
                        return Err(HistoryError::Discontinuity);
                    }
                }
            }
        }
        Ok(HitFrame {
            tick,
            scene,
            poses: poses.into_boxed_slice(),
        })
    }
}
