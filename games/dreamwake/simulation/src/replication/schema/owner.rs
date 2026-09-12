//! Complete owner projection; all latent fields have explicit stable IDs.
use super::*;
use crate::state;
use engine_net::codec::BoundedVec;
engine_net::schema! {pub(super) struct Body(SchemaId(0x120b),1,Representation::OwnerCheckpoint) {
1=>id:u64=1=>o(Units::Unitless,U64),
2=>shards:u32=0=>o(Units::Unitless,U32),
3=>combo:u32=0=>o(Units::Unitless,U32),
4=>hit_flash:f32=0.0=>o(Units::Seconds,NONNEG),
5=>attack_flash:f32=0.0=>o(Units::Seconds,NONNEG),
6=>attack_power:f32=1.0=>o(Units::Unitless,NONNEG),
7=>ability_power:f32=1.0=>o(Units::Unitless,NONNEG),
8=>movement_speed:f32=7.8=>o(Units::Custom("metres/second"),NONNEG),
9=>critical_chance:f32=0.08=>o(Units::Unitless,NONNEG),
10=>recovery:f32=1.0=>o(Units::Unitless,NONNEG),
11=>defense:f32=0.0=>o(Units::Unitless,F),
}}
impl From<&state::HeroBody> for Body {
    fn from(v: &state::HeroBody) -> Self {
        Self {
            id: v.id,
            shards: v.shards,
            combo: v.combo,
            hit_flash: v.hit_flash,
            attack_flash: v.attack_flash,
            attack_power: v.attack_power,
            ability_power: v.ability_power,
            movement_speed: v.movement_speed,
            critical_chance: v.critical_chance,
            recovery: v.recovery,
            defense: v.defense,
        }
    }
}
impl From<Body> for state::HeroBody {
    fn from(v: Body) -> Self {
        Self {
            id: v.id,
            shards: v.shards,
            combo: v.combo,
            hit_flash: v.hit_flash,
            attack_flash: v.attack_flash,
            attack_power: v.attack_power,
            ability_power: v.ability_power,
            movement_speed: v.movement_speed,
            critical_chance: v.critical_chance,
            recovery: v.recovery,
            defense: v.defense,
        }
    }
}
engine_net::schema! {pub(super) struct CombatIdentity(SchemaId(0x120c),1,Representation::OwnerCheckpoint) {
1=>generation:u32=1=>o(Units::Unitless,U32),
2=>segment:u64=1=>o(Units::Unitless,U64),
3=>pose_revision:u64=1=>o(Units::Unitless,U64),
4=>tick:u32=0=>o(Units::Ticks,U32),
}}
impl From<&state::CombatIdentity> for CombatIdentity {
    fn from(v: &state::CombatIdentity) -> Self {
        Self {
            generation: v.generation,
            segment: v.segment,
            pose_revision: v.pose_revision,
            tick: v.tick,
        }
    }
}
impl From<CombatIdentity> for state::CombatIdentity {
    fn from(v: CombatIdentity) -> Self {
        Self {
            generation: v.generation,
            segment: v.segment,
            pose_revision: v.pose_revision,
            tick: v.tick,
        }
    }
}
// Lossless columns avoid repeating five field headers for every shield episode.
// Runtime state remains the engine/game episode collection, never parallel arrays.
engine_net::schema! {pub(super) struct DefenseEpisodes(SchemaId(0x120e),2,Representation::OwnerCheckpoint) {
1=>next_episode:u64=1=>o(Units::Unitless,U64),
2=>ids:BoundedVec<u64,64>=BoundedVec::default()=>o(Units::Unitless,U64),
3=>activated_ticks:BoundedVec<u32,64>=BoundedVec::default()=>o(Units::Ticks,U32),
4=>expires_ticks:BoundedVec<u32,64>=BoundedVec::default()=>o(Units::Ticks,U32),
5=>granted:BoundedVec<f32,64>=BoundedVec::default()=>o(Units::Unitless,NONNEG),
6=>spent:BoundedVec<f32,64>=BoundedVec::default()=>o(Units::Unitless,NONNEG),
}}
impl TryFrom<&state::DefenseEpisodes> for DefenseEpisodes {
    type Error = SchemaError;
    fn try_from(v: &state::DefenseEpisodes) -> Result<Self, SchemaError> {
        Ok(Self {
            next_episode: v.next_episode,
            ids: BoundedVec::new(v.episodes.iter().map(|e| e.id).collect()).map_err(invalid)?,
            activated_ticks: BoundedVec::new(v.episodes.iter().map(|e| e.activated_tick).collect())
                .map_err(invalid)?,
            expires_ticks: BoundedVec::new(v.episodes.iter().map(|e| e.expires_tick).collect())
                .map_err(invalid)?,
            granted: BoundedVec::new(v.episodes.iter().map(|e| e.granted).collect())
                .map_err(invalid)?,
            spent: BoundedVec::new(v.episodes.iter().map(|e| e.spent).collect())
                .map_err(invalid)?,
        })
    }
}
impl TryFrom<DefenseEpisodes> for state::DefenseEpisodes {
    type Error = SchemaError;
    fn try_from(v: DefenseEpisodes) -> Result<Self, Self::Error> {
        let count = v.ids.as_slice().len();
        if [
            v.activated_ticks.as_slice().len(),
            v.expires_ticks.as_slice().len(),
            v.granted.as_slice().len(),
            v.spent.as_slice().len(),
        ]
        .into_iter()
        .any(|n| n != count)
        {
            return Err(SchemaError::InvalidEncoding);
        }
        Ok(Self {
            next_episode: v.next_episode,
            episodes: (0..count)
                .map(|i| state::ShieldEpisode {
                    id: v.ids.as_slice()[i],
                    activated_tick: v.activated_ticks.as_slice()[i],
                    expires_tick: v.expires_ticks.as_slice()[i],
                    granted: v.granted.as_slice()[i],
                    spent: v.spent.as_slice()[i],
                })
                .collect(),
        })
    }
}
engine_net::schema_enum! {pub(super) enum RewardKind {1=>Memory(MemoryKind),2=>Essence(Essence),3=>Upgrade(Upgrade)}}
impl From<&crate::RewardKind> for RewardKind {
    fn from(v: &crate::RewardKind) -> Self {
        match v {
            crate::RewardKind::Memory(x) => Self::Memory((*x).into()),
            crate::RewardKind::Essence(x) => Self::Essence((*x).into()),
            crate::RewardKind::Upgrade(x) => Self::Upgrade((*x).into()),
        }
    }
}
impl From<RewardKind> for crate::RewardKind {
    fn from(v: RewardKind) -> Self {
        match v {
            RewardKind::Memory(x) => Self::Memory(x.into()),
            RewardKind::Essence(x) => Self::Essence(x.into()),
            RewardKind::Upgrade(x) => Self::Upgrade(x.into()),
        }
    }
}
engine_net::schema! {pub(super) struct Reward(SchemaId(0x120f),1,Representation::OwnerCheckpoint) {
1=>kind:RewardKind=RewardKind::Memory(MemoryKind::Crescent)=>o(Units::Unitless,Range::None),
2=>rarity:Rarity=Rarity::Common=>o(Units::Unitless,Range::None),
3=>title:BoundedString<128>=BoundedString::default()=>o(Units::Unitless,Range::None),
4=>description:BoundedString<512>=BoundedString::default()=>o(Units::Unitless,Range::None),
}}
impl TryFrom<&crate::Reward> for Reward {
    type Error = SchemaError;
    fn try_from(v: &crate::Reward) -> Result<Self, SchemaError> {
        Ok(Self {
            kind: (&v.kind).into(),
            rarity: v.rarity.into(),
            title: BoundedString::new(&v.title)?,
            description: BoundedString::new(&v.description)?,
        })
    }
}
impl From<Reward> for crate::Reward {
    fn from(v: Reward) -> Self {
        Self {
            kind: v.kind.into(),
            rarity: v.rarity.into(),
            title: v.title.as_str().into(),
            description: v.description.as_str().into(),
        }
    }
}
engine_net::schema! {pub(super) struct Actor(SchemaId(0x1210),1,Representation::OwnerCheckpoint) {
1=>view:Body=Body::defaults()=>o(Units::Unitless,Range::None),
2=>active:bool=true=>o(Units::Unitless,Range::None),
3=>critical_rng:Random=Random::defaults()=>o(Units::Unitless,Range::None),
4=>ready:bool=false=>o(Units::Unitless,Range::None),
5=>rewards:BoundedVec<Reward,8>=BoundedVec::default()=>o(Units::Unitless,Range::None),
}}
impl TryFrom<&state::Hero> for Actor {
    type Error = SchemaError;
    fn try_from(v: &state::Hero) -> Result<Self, SchemaError> {
        Ok(Self {
            view: (&v.view).into(),
            active: v.active,
            critical_rng: (&v.critical_rng).into(),
            ready: v.ready,
            rewards: BoundedVec::new(
                v.rewards
                    .iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()?,
            )
            .map_err(invalid)?,
        })
    }
}
impl From<Actor> for state::Hero {
    fn from(v: Actor) -> Self {
        Self {
            view: v.view.into(),
            active: v.active,
            critical_rng: v.critical_rng.into(),
            ready: v.ready,
            rewards: v.rewards.into_vec().into_iter().map(Into::into).collect(),
        }
    }
}
engine_net::schema! {pub(super) struct RayKey(SchemaId(0x1212),1,Representation::OwnerCheckpoint) {
1=>match_epoch:u32=1=>o(Units::Unitless,U32),
2=>connection_epoch:u64=0=>o(Units::Unitless,U64),
3=>command_stream:u32=0=>o(Units::Unitless,U32),
4=>ownership_epoch:u32=0=>o(Units::Unitless,U32),
5=>actor:u64=1=>o(Units::Unitless,U64),
6=>actor_generation:u32=1=>o(Units::Unitless,U32),
7=>command_sequence:u64=1=>o(Units::Unitless,U64),
8=>action_slot:u8=0=>o(Units::Unitless,U8),
}}
impl From<crate::combat::RayActionKey> for RayKey {
    fn from(v: crate::combat::RayActionKey) -> Self {
        Self {
            match_epoch: v.match_epoch,
            connection_epoch: v.connection_epoch,
            command_stream: v.command_stream,
            ownership_epoch: v.ownership_epoch,
            actor: v.actor,
            actor_generation: v.actor_generation,
            command_sequence: v.command_sequence,
            action_slot: v.action_slot,
        }
    }
}
impl From<RayKey> for crate::combat::RayActionKey {
    fn from(v: RayKey) -> Self {
        Self {
            match_epoch: v.match_epoch,
            connection_epoch: v.connection_epoch,
            command_stream: v.command_stream,
            ownership_epoch: v.ownership_epoch,
            actor: v.actor,
            actor_generation: v.actor_generation,
            command_sequence: v.command_sequence,
            action_slot: v.action_slot,
        }
    }
}
engine_net::schema! {pub(super) struct Ammo(SchemaId(0x1213),1,Representation::OwnerCheckpoint) {
1=>current:f32=6.0=>o(Units::Unitless,Range::Float32{minimum:0.0,maximum:6.0}),
2=>capacity:f32=6.0=>o(Units::Unitless,Range::Float32{minimum:6.0,maximum:6.0}),
}}
engine_net::schema! {pub(super) struct BeamSample(SchemaId(0x1215),2,Representation::OwnerCheckpoint) {
1=>key:RayKey=RayKey::defaults()=>o(Units::Unitless,Range::None),
2=>execution_server_tick:u64=1=>o(Units::Ticks,U64),
3=>query_server_tick:u64=1=>o(Units::Ticks,U64),
4=>query_gameplay_tick:u32=0=>o(Units::Ticks,U32),
5=>accepted_gameplay_tick:u32=0=>o(Units::Ticks,U32),
6=>aim:[f32;2]=[1.0,0.0]=>o(Units::Unitless,F),
7=>query_fraction:u16=0=>o(Units::Unitless,U16),
}}
impl From<&state::BeamSample> for BeamSample {
    fn from(v: &state::BeamSample) -> Self {
        Self {
            key: v.key.into(),
            execution_server_tick: v.execution_server_tick,
            query_server_tick: v.query_server_tick,
            query_gameplay_tick: v.query_gameplay_tick,
            query_fraction: v.query_fraction,
            accepted_gameplay_tick: v.accepted_gameplay_tick,
            aim: v.aim,
        }
    }
}
impl From<BeamSample> for state::BeamSample {
    fn from(v: BeamSample) -> Self {
        Self {
            key: v.key.into(),
            execution_server_tick: v.execution_server_tick,
            query_server_tick: v.query_server_tick,
            query_gameplay_tick: v.query_gameplay_tick,
            query_fraction: v.query_fraction,
            accepted_gameplay_tick: v.accepted_gameplay_tick,
            aim: v.aim,
        }
    }
}
engine_net::schema! {pub(super) struct BeamEpisode(SchemaId(0x1216),1,Representation::OwnerCheckpoint) {
1=>key:RayKey=RayKey::defaults()=>o(Units::Unitless,Range::None),
2=>started_tick:u32=0=>o(Units::Ticks,U32),
3=>last_damage_tick:u32=0=>o(Units::Ticks,U32),
4=>sample:BeamSample=BeamSample::defaults()=>o(Units::Unitless,Range::None),
}}
impl From<&state::BeamEpisode> for BeamEpisode {
    fn from(v: &state::BeamEpisode) -> Self {
        Self {
            key: v.key.into(),
            started_tick: v.started_tick,
            last_damage_tick: v.last_damage_tick,
            sample: (&v.sample).into(),
        }
    }
}
impl From<BeamEpisode> for state::BeamEpisode {
    fn from(v: BeamEpisode) -> Self {
        Self {
            key: v.key.into(),
            started_tick: v.started_tick,
            last_damage_tick: v.last_damage_tick,
            sample: v.sample.into(),
        }
    }
}
engine_net::schema! {pub(super) struct Ray(SchemaId(0x1214),1,Representation::OwnerCheckpoint) {
1=>ammo:Ammo=Ammo::defaults()=>o(Units::Unitless,Range::None),
2=>cooldown:f32=0.0=>o(Units::Seconds,Range::Float32{minimum:0.0,maximum:0.5}),
3=>last_key:Option<RayKey>=None=>o(Units::Unitless,Range::None),
4=>beam:Option<BeamEpisode>=None=>o(Units::Unitless,Range::None),
}}
impl From<&state::RayState> for Ray {
    fn from(v: &state::RayState) -> Self {
        Self {
            ammo: Ammo {
                current: v.ammo.current(),
                capacity: v.ammo.capacity(),
            },
            cooldown: v.cooldown,
            last_key: v.last_key.map(Into::into),
            beam: v.beam.as_ref().map(Into::into),
        }
    }
}
impl TryFrom<Ray> for state::RayState {
    type Error = SchemaError;
    fn try_from(v: Ray) -> Result<Self, SchemaError> {
        Ok(Self {
            ammo: engine_core::Meter::new(v.ammo.current, v.ammo.capacity)
                .ok_or(SchemaError::InvalidEncoding)?,
            cooldown: v.cooldown,
            last_key: v.last_key.map(Into::into),
            beam: v.beam.map(Into::into),
        })
    }
}
engine_net::schema! {pub(super) struct Charge(SchemaId(0x1223),1,Representation::OwnerCheckpoint) {
1=>episode:u64=0=>o(Units::Unitless,U64),
2=>phase:u8=0=>o(Units::Unitless,U8),
3=>ticks:u16=0=>o(Units::Ticks,U16),
4=>cooldown:u16=0=>o(Units::Ticks,U16),
5=>curve_id:u16=0=>o(Units::Unitless,U16),
6=>curve_version:u16=0=>o(Units::Unitless,U16),
7=>direction:[f32;2]=[0.0;2]=>o(Units::Unitless,F),
8=>distance:f32=0.0=>o(Units::Metres,NONNEG),
9=>stamina:f32=100.0=>o(Units::Unitless,NONNEG),
10=>begin_key:Option<RayKey>=None=>o(Units::Unitless,Range::None),
}}
impl From<&state::ChargedMovement> for Charge {
    fn from(v: &state::ChargedMovement) -> Self {
        let mut result = Self {
            episode: v.state.episode,
            cooldown: v.state.cooldown_ticks,
            stamina: v.stamina.current(),
            begin_key: v.begin_key.map(Into::into),
            ..Self::defaults()
        };
        match v.state.phase {
            engine_core::ChargePhase::Idle => {}
            engine_core::ChargePhase::Charging { ticks } => {
                result.phase = 1;
                result.ticks = ticks;
            }
            engine_core::ChargePhase::Executing {
                curve,
                cursor,
                direction,
                distance,
            } => {
                result.phase = 2;
                result.ticks = cursor;
                result.curve_id = curve.id;
                result.curve_version = curve.version;
                result.direction = direction;
                result.distance = distance;
            }
        }
        result
    }
}
impl TryFrom<Charge> for state::ChargedMovement {
    type Error = SchemaError;
    fn try_from(v: Charge) -> Result<Self, SchemaError> {
        let phase = match v.phase {
            0 if v.ticks == 0
                && v.curve_id == 0
                && v.curve_version == 0
                && v.direction == [0.0; 2]
                && v.distance == 0.0 =>
            {
                engine_core::ChargePhase::Idle
            }
            1 if v.curve_id == 0
                && v.curve_version == 0
                && v.direction == [0.0; 2]
                && v.distance == 0.0 =>
            {
                engine_core::ChargePhase::Charging { ticks: v.ticks }
            }
            2 => engine_core::ChargePhase::Executing {
                curve: engine_core::MotionCurveKey {
                    id: v.curve_id,
                    version: v.curve_version,
                },
                cursor: v.ticks,
                direction: v.direction,
                distance: v.distance,
            },
            _ => return Err(SchemaError::InvalidEncoding),
        };
        Ok(Self {
            state: engine_core::ChargeState {
                episode: v.episode,
                phase,
                cooldown_ticks: v.cooldown,
            },
            stamina: engine_core::Meter::new(v.stamina, 100.0)
                .ok_or(SchemaError::InvalidEncoding)?,
            begin_key: v.begin_key.map(Into::into),
        })
    }
}
engine_net::schema! {pub(super) struct SavedHero(SchemaId(0x1211),1,Representation::OwnerCheckpoint) {
1=>actor:Actor=Actor::defaults()=>o(Units::Unitless,Range::None),
2=>health:Health=Health::defaults()=>o(Units::Unitless,Range::None),
3=>combat:Combat=Combat::defaults()=>o(Units::Unitless,Range::None),
4=>combat_identity:CombatIdentity=CombatIdentity::defaults()=>o(Units::Unitless,Range::None),
5=>defense_episodes:DefenseEpisodes=DefenseEpisodes::defaults()=>o(Units::Unitless,Range::None),
6=>motion:Motion=Motion::defaults()=>o(Units::Unitless,Range::None),
7=>action:Action=Action::defaults()=>o(Units::Unitless,Range::None),
8=>loadout:BoundedVec<Memory,4>=BoundedVec::new(state::SavedHero::new(1,0).loadout.0.iter().map(Into::into).collect()).unwrap()=>o(Units::Unitless,Range::None),
9=>progression:Progression=Progression::defaults()=>o(Units::Unitless,Range::None),
10=>ray:Ray=Ray::defaults()=>o(Units::Unitless,Range::None),
11=>charge:Charge=Charge::defaults()=>o(Units::Unitless,Range::None),
}}
impl TryFrom<&state::SavedHero> for SavedHero {
    type Error = SchemaError;
    fn try_from(v: &state::SavedHero) -> Result<Self, SchemaError> {
        Ok(Self {
            actor: (&v.actor).try_into()?,
            health: (&v.health).into(),
            combat: (&v.combat).try_into()?,
            combat_identity: (&v.combat_identity).into(),
            defense_episodes: (&v.defense_episodes).try_into()?,
            motion: (&v.motion).into(),
            action: (&v.action).into(),
            loadout: BoundedVec::new(v.loadout.0.iter().map(Into::into).collect())
                .map_err(invalid)?,
            progression: (&v.progression).into(),
            ray: (&v.ray).into(),
            charge: (&v.charge).into(),
        })
    }
}
impl TryFrom<SavedHero> for state::SavedHero {
    type Error = SchemaError;
    fn try_from(v: SavedHero) -> Result<Self, SchemaError> {
        Ok(Self {
            actor: v.actor.into(),
            health: v.health.into(),
            combat: v.combat.try_into()?,
            combat_identity: v.combat_identity.into(),
            defense_episodes: v.defense_episodes.try_into()?,
            motion: v.motion.into(),
            action: v.action.into(),
            loadout: engine_core::Loadout(
                v.loadout.into_vec().into_iter().map(Into::into).collect(),
            ),
            progression: v.progression.into(),
            ray: v.ray.try_into()?,
            charge: v.charge.try_into()?,
        })
    }
}
engine_net::schema! {pub(super) struct HeldInput(SchemaId(0x1224),1,Representation::OwnerCheckpoint) {
1=>movement:[f32;2]=[0.0;2]=>o(Units::Unitless,F),
2=>aim:[f32;2]=[0.0;2]=>o(Units::Unitless,F),
3=>attack:bool=false=>o(Units::Unitless,Range::None),
}}
impl From<&crate::DreamInput> for HeldInput {
    fn from(input: &crate::DreamInput) -> Self {
        Self {
            movement: input.movement,
            aim: input.aim,
            attack: input.attack,
        }
    }
}
impl From<HeldInput> for crate::DreamInput {
    fn from(input: HeldInput) -> Self {
        Self {
            movement: input.movement,
            aim: input.aim,
            attack: input.attack,
            ..Default::default()
        }
    }
}
engine_net::schema! {pub(super) struct InputContinuity(SchemaId(0x1225),1,Representation::OwnerCheckpoint) {
1=>last_input:Option<HeldInput>=None=>o(Units::Unitless,Range::None),
2=>missing_streak:u32=0=>o(Units::Ticks,U32),
}}
impl From<&engine_net::commands::InputContinuity<crate::DreamInput>> for InputContinuity {
    fn from(value: &engine_net::commands::InputContinuity<crate::DreamInput>) -> Self {
        Self {
            last_input: value.last_input.as_ref().map(Into::into),
            missing_streak: value.missing_streak,
        }
    }
}
impl From<InputContinuity> for engine_net::commands::InputContinuity<crate::DreamInput> {
    fn from(value: InputContinuity) -> Self {
        Self {
            last_input: value.last_input.map(Into::into),
            missing_streak: value.missing_streak,
        }
    }
}
engine_net::schema! {pub(super) struct Owner(SchemaId(0x1102),4,Representation::OwnerCheckpoint) {
1=>schema_version:u32=crate::replication::OWNER_CHECKPOINT_SCHEMA=>o(Units::Unitless,U32),
2=>ruleset:BoundedString<64>=BoundedString::new(crate::CheckpointIdentity::current(1).ruleset).unwrap()=>o(Units::Unitless,Range::None),
3=>owner:u64=1=>o(Units::Unitless,U64),
4=>ownership_revision:u32=1=>o(Units::Unitless,U32),
5=>fixed_step_hz:u32=crate::TICK_HZ=>o(Units::Unitless,U32),
6=>collision_identity:[u8;32]=crate::collision::CollisionManifest::current(1).identity()=>o(Units::Unitless,U8),
7=>context:Global=Global::defaults()=>o(Units::Unitless,Range::None),
8=>hero:SavedHero=SavedHero::defaults()=>o(Units::Unitless,Range::None),
10=>starfall:super::starfall::Domain=super::starfall::Domain::defaults()=>o(Units::Unitless,Range::None),
11=>required_bases:BoundedVec<ColliderKey,1>=BoundedVec::default()=>o(Units::Unitless,Range::None),
12=>input_continuity:InputContinuity=InputContinuity::defaults()=>o(Units::Unitless,Range::None),
}}
impl Root for Owner {
    type Model = crate::replication::OwnerCheckpoint;
    fn capture(v: &Self::Model) -> Result<Self, SchemaError> {
        v.validate_for(v.expectation()).map_err(invalid)?;
        Ok(Self {
            schema_version: v.schema,
            ruleset: BoundedString::new(&v.ruleset)?,
            owner: v.owner,
            ownership_revision: v.ownership_revision,
            fixed_step_hz: v.fixed_step_hz,
            collision_identity: v.collision_identity,
            context: Global::capture(&v.context)?,
            hero: (&v.hero).try_into()?,
            starfall: (&v.starfall).try_into()?,
            required_bases: BoundedVec::new(
                v.required_bases.iter().copied().map(Into::into).collect(),
            )
            .map_err(invalid)?,
            input_continuity: (&v.input_continuity).into(),
        })
    }
    fn restore(&self) -> Result<Self::Model, SchemaError> {
        let value = Self::Model {
            schema: self.schema_version,
            ruleset: self.ruleset.as_str().into(),
            owner: self.owner,
            ownership_revision: self.ownership_revision,
            fixed_step_hz: self.fixed_step_hz,
            collision_identity: self.collision_identity,
            context: self.context.restore()?,
            hero: self.hero.clone().try_into()?,
            starfall: self.starfall.clone().into(),
            required_bases: self
                .required_bases
                .clone()
                .into_vec()
                .into_iter()
                .map(Into::into)
                .collect(),
            input_continuity: self.input_continuity.clone().into(),
        };
        value.validate_for(value.expectation()).map_err(invalid)?;
        Ok(value)
    }
}
