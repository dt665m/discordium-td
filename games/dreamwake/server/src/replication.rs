//! Dreamwake's trusted adapter from one committed capture to sparse peer scopes.
use dreamwake_protocol::replication::{
    ReplicaPayload, decode_replica, encode_replica_with_compression,
};
use dreamwake_sim::replication::{
    ApprovedSources, CommittedReplication, PublicReplica, ReplicationKey,
};
use engine_net::{interest::*, replication::ScopeLimits, replication::ServerScopes, types::*};
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Debug)]
pub struct AdapterError(pub String);
impl std::fmt::Display for AdapterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for AdapterError {}
fn error(e: impl std::fmt::Debug) -> AdapterError {
    AdapterError(format!("replication adapter: {e:?}"))
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Key {
    Global,
    Collision,
    Owner(u64),
    Actor(ReplicationKey),
}
struct ActorContent {
    value: PublicReplica,
    sampled: ServerTick,
}
struct Peer {
    player: u64,
    ownership: OwnershipEpoch,
    sendable: bool,
    sources: BTreeSet<u64>,
    epoch: ConnectionEpoch,
    ready: SceneRevision,
    previous: BTreeSet<EntityId>,
    eligible: Option<EligibleSet>,
    scopes: ServerScopes<Vec<u8>>,
}
/// Live actors retain stable slots. Retired actor slots may be reassigned only
/// with a strictly newer generation, fencing old state and lifecycle messages.
pub struct DreamReplication {
    graph: Graph,
    epoch: u32,
    cull_radius: f64,
    ids: HashMap<Key, EntityId>,
    keys: BTreeMap<EntityId, Key>,
    present: BTreeSet<EntityId>,
    retired: BTreeSet<EntityId>,
    peers: BTreeMap<u64, Peer>,
    capture: Option<CommittedReplication>,
    max_identities: usize,
    actor_content: HashMap<ReplicationKey, ActorContent>,
    owner_bases: BTreeMap<u64, BTreeSet<EntityId>>,
    canonical: CanonicalPayloadCache,
    compression: engine_net::CompressionPolicy,
}
const POLICY: PolicyRevision = PolicyRevision(1);
struct Policy {
    connection: ConnectionId,
    owner: Option<EntityId>,
    collision: EntityId,
    bases: BTreeSet<EntityId>,
}
impl DisclosurePolicy for Policy {
    fn revision(&self) -> PolicyRevision {
        POLICY
    }
    fn representation(
        &self,
        connection: ConnectionId,
        entity: &EntityRegistration,
    ) -> Option<RepresentationGrant> {
        if matches!(entity.visibility, Visibility::Owner(owner) if owner != connection) {
            return None;
        }
        Some(RepresentationGrant {
            schema_id: 1,
            revision: RepresentationRevision(1),
            fields: 1,
            prediction_allowed: self.bases.contains(&entity.id)
                || entity.routes.global
                || matches!(entity.visibility, Visibility::Owner(_)),
        })
    }
    fn permits_dependency(
        &self,
        connection: ConnectionId,
        source: EntityId,
        target: EntityId,
    ) -> bool {
        connection == self.connection
            && self.owner == Some(source)
            && (target == self.collision || self.bases.contains(&target))
    }
}
struct PawnView {
    owner: ConnectionId,
    expected: ConnectionView,
}
impl ConnectionAuthorizer for PawnView {
    fn authorize(&self, owner: ConnectionId, view: &ConnectionView) -> bool {
        owner == self.owner && *view == self.expected
    }
}
impl DreamReplication {
    #[cfg(test)]
    pub fn new(epoch: u32, cull_radius: f64, limits: Limits) -> Result<Self, AdapterError> {
        Self::with_compression(
            epoch,
            cull_radius,
            limits,
            engine_net::CompressionPolicy::default(),
        )
    }
    pub fn with_compression(
        epoch: u32,
        cull_radius: f64,
        limits: Limits,
        compression: engine_net::CompressionPolicy,
    ) -> Result<Self, AdapterError> {
        if epoch == 0 || !cull_radius.is_finite() || cull_radius < 0.0 {
            return Err(error("invalid match/cull radius"));
        }
        let max_identities = limits.max_entities.min(8192);
        Ok(Self {
            graph: Graph::new(limits).map_err(error)?,
            epoch,
            cull_radius,
            ids: HashMap::new(),
            keys: BTreeMap::new(),
            present: BTreeSet::new(),
            retired: BTreeSet::new(),
            peers: BTreeMap::new(),
            capture: None,
            max_identities,
            actor_content: HashMap::new(),
            owner_bases: BTreeMap::new(),
            canonical: CanonicalPayloadCache::new(max_identities, 2 * 1024 * 1024),
            compression,
        })
    }
    fn identity(&mut self, key: Key) -> Result<EntityId, AdapterError> {
        if let Some(&old) = self.ids.get(&key) {
            if self.retired.remove(&old) {
                let id = EntityId {
                    index: old.index,
                    generation: old
                        .generation
                        .checked_add(1)
                        .ok_or_else(|| error("entity generation exhausted"))?,
                };
                self.ids.insert(key, id);
                self.keys.remove(&old);
                self.keys.insert(id, key);
                return Ok(id);
            }
            return Ok(old);
        }
        if let Some(old) = self
            .retired
            .iter()
            .copied()
            .find(|id| matches!(self.keys.get(id), Some(Key::Actor(_))))
        {
            let id = EntityId {
                index: old.index,
                generation: old
                    .generation
                    .checked_add(1)
                    .ok_or_else(|| error("entity generation exhausted"))?,
            };
            let previous_key = self
                .keys
                .remove(&old)
                .ok_or_else(|| error("missing retired identity"))?;
            self.ids.remove(&previous_key);
            self.retired.remove(&old);
            self.ids.insert(key, id);
            self.keys.insert(id, key);
            return Ok(id);
        }
        if self.ids.len() >= self.max_identities {
            return Err(error("match identity capacity requires reset"));
        }
        let id = EntityId {
            index: self.ids.len() as u64 + 1,
            generation: self.epoch,
        };
        self.ids.insert(key, id);
        self.keys.insert(id, key);
        Ok(id)
    }
    /// Called only after authenticated admission and an accepted ready-scene
    /// handshake. No client position or observer subscription is accepted here.
    #[cfg(test)]
    pub fn set_peer(
        &mut self,
        owner: u64,
        epoch: ConnectionEpoch,
        ready: SceneRevision,
    ) -> Result<(), AdapterError> {
        self.set_player_peer(owner, owner, OwnershipEpoch(1), epoch, ready)
    }
    pub fn set_player_peer(
        &mut self,
        connection: u64,
        player: u64,
        ownership: OwnershipEpoch,
        epoch: ConnectionEpoch,
        ready: SceneRevision,
    ) -> Result<(), AdapterError> {
        if connection == 0 || player == 0 || ownership.0 == 0 || epoch.0 == 0 || ready.0 == 0 {
            return Err(error("invalid peer"));
        }
        if let Some(peer) = self.peers.get_mut(&connection) {
            if peer.epoch == epoch && peer.player == player && peer.ownership == ownership {
                peer.ready = ready;
                return Ok(());
            }
            return Err(error("disconnect old connection incarnation first"));
        }
        if self.peers.len() >= self.graph.limits().max_connections
            || self.peers.values().any(|peer| peer.player == player)
        {
            return Err(error("peer capacity or player already owned"));
        }
        self.peers.insert(
            connection,
            Peer {
                player,
                ownership,
                sendable: false,
                sources: BTreeSet::new(),
                epoch,
                ready,
                previous: BTreeSet::new(),
                eligible: None,
                scopes: ServerScopes::new(
                    ConnectionId(connection),
                    epoch,
                    self.graph.accounting().world_revision,
                    POLICY,
                    ScopeLimits::default(),
                )
                .map_err(error)?,
            },
        );
        Ok(())
    }
    pub fn disconnect(&mut self, owner: u64, tick: ServerTick) -> Result<(), AdapterError> {
        self.graph
            .remove_connection(tick, ConnectionId(owner))
            .map_err(error)?;
        self.peers.remove(&owner);
        Ok(())
    }
    pub fn reserve_owner(
        &mut self,
        owner: u64,
    ) -> Result<(EntityId, EntityId, EntityId), AdapterError> {
        if owner == 0 {
            return Err(error("invalid owner"));
        }
        Ok((
            self.identity(Key::Global)?,
            self.identity(Key::Owner(owner))?,
            self.identity(Key::Collision)?,
        ))
    }
    pub fn global_entity(&self) -> Option<EntityId> {
        self.ids.get(&Key::Global).copied()
    }
    pub fn collision_entity(&self) -> Option<EntityId> {
        self.ids.get(&Key::Collision).copied()
    }
    pub fn owner_entity(&self, owner: u64) -> Option<EntityId> {
        self.ids.get(&Key::Owner(owner)).copied()
    }
    /// Bind a simulation-owned projectile or explicitly reserved delayed spawn.
    /// Reservations have identity/liveness but no public graph registration.
    /// The next capture must declare them or retire their generation.
    pub(crate) fn reserve_projectile_identity(
        &mut self,
        id: u64,
    ) -> Result<EntityId, AdapterError> {
        let entity = self.identity(Key::Actor(ReplicationKey::Projectile(id)))?;
        self.present.insert(entity);
        Ok(entity)
    }
    pub fn owner_group_entities(&self, owner: u64) -> Result<Vec<EntityId>, AdapterError> {
        let mut members = vec![
            self.global_entity()
                .ok_or_else(|| error("missing global"))?,
            self.owner_entity(owner)
                .ok_or_else(|| error("missing owner"))?,
            self.collision_entity()
                .ok_or_else(|| error("missing collision"))?,
        ];
        members.extend(self.owner_bases.get(&owner).into_iter().flatten().copied());
        if !dreamwake_protocol::live::baselines::valid_member_count(members.len()) {
            return Err(error("owner dependency capacity"));
        }
        Ok(members)
    }
    #[cfg(test)]
    pub fn actor_entity(&self, key: ReplicationKey) -> Option<EntityId> {
        self.ids.get(&Key::Actor(key)).copied()
    }
    pub fn eligible(&self, owner: u64) -> Option<&EligibleSet> {
        self.peers.get(&owner)?.eligible.as_ref()
    }
    pub fn scopes_mut(&mut self, owner: u64) -> Option<&mut ServerScopes<Vec<u8>>> {
        let peer = self.peers.get_mut(&owner)?;
        peer.sendable.then_some(&mut peer.scopes)
    }
    pub fn world_revision(&self) -> u64 {
        self.graph.accounting().world_revision
    }
    pub fn policy_revision(&self) -> PolicyRevision {
        POLICY
    }
    #[cfg(test)]
    pub fn canonical_cache_stats(&self) -> CanonicalCacheStats {
        self.canonical.stats()
    }
    #[cfg(test)]
    pub fn accounting(&self) -> Accounting {
        self.graph.accounting()
    }

    /// Refresh graph once from one committed authority capture, then freeze and
    /// gather every admitted peer. Only sparse candidates are projected per peer.
    pub fn prepare(
        &mut self,
        capture: CommittedReplication,
        active_owners: &[u64],
    ) -> Result<(), AdapterError> {
        self.canonical.clear();
        for peer in self.peers.values_mut() {
            peer.sendable = false;
            peer.eligible = None;
        }
        let active_owners: BTreeSet<u64> = active_owners.iter().copied().collect();
        let stamp = capture.global().stamp;
        if stamp.match_epoch != self.epoch {
            return Err(error("wrong match epoch"));
        }
        let scene = SceneRevision(u32::try_from(stamp.scene_revision).map_err(error)?);
        let tick = ServerTick(stamp.server_tick);
        let collision = capture.collision();
        if collision.scene_revision() != stamp.scene_revision
            || collision.identity()
                != dreamwake_protocol::live::ContentIdentity::current(scene).scene
        {
            return Err(error(
                "collision content differs from advertised ready scene",
            ));
        }
        let collision_entity = self.identity(Key::Collision)?;
        let mut registrations = Vec::new();
        registrations.push((
            Key::Global,
            [0.0, 0.0],
            Routes {
                global: true,
                ..Default::default()
            },
            Visibility::Public,
        ));
        registrations.push((
            Key::Collision,
            [0.0, 0.0],
            Routes {
                global: true,
                ..Default::default()
            },
            Visibility::Public,
        ));
        for &owner in &active_owners {
            let Some((&connection, _)) = self.peers.iter().find(|(_, peer)| peer.player == owner)
            else {
                continue;
            };
            if capture.position(ReplicationKey::Hero(owner)).is_some() {
                registrations.push((
                    Key::Owner(owner),
                    [0.0, 0.0],
                    Routes {
                        owner: Some(ConnectionId(connection)),
                        ..Default::default()
                    },
                    Visibility::Owner(ConnectionId(connection)),
                ));
            }
        }
        for key in capture.keys() {
            if let Some(position) = capture.position(key) {
                registrations.push((
                    Key::Actor(key),
                    position,
                    Routes {
                        spatial: Some(SpatialRoute::DormancyDriven),
                        ..Default::default()
                    },
                    Visibility::Public,
                ));
            }
        }
        if registrations.len() > self.max_identities {
            return Err(error("capture entity capacity"));
        }
        let all_sources =
            ApprovedSources::new(capture.keys().into_iter().filter_map(|key| match key {
                ReplicationKey::Hero(id) | ReplicationKey::Enemy(id) => Some(id),
                _ => None,
            }))
            .map_err(error)?;
        // Resolve only positive typed platform views, before owner registrations.
        // A missing declared base fails the complete capture rather than silently
        // admitting an owner checkpoint without its simulation prerequisite.
        let mut platform_ids = BTreeMap::new();
        for key in capture.keys() {
            if let ReplicationKey::Platform(_) = key {
                if let Some(PublicReplica::Platform(view)) = capture.project(key, &all_sources) {
                    if platform_ids
                        .insert(view.collider, self.identity(Key::Actor(key))?)
                        .is_some()
                    {
                        return Err(error("duplicate platform collider identity"));
                    }
                }
            }
        }
        let mut owner_bases = BTreeMap::new();
        for &owner in &active_owners {
            let required = capture.required_bases(owner);
            if required.len() > 1 {
                return Err(error("owner base dependency capacity"));
            }
            let bases = required
                .into_iter()
                .map(|key| {
                    platform_ids
                        .get(&key)
                        .copied()
                        .ok_or_else(|| error("missing required platform"))
                })
                .collect::<Result<BTreeSet<_>, _>>()?;
            owner_bases.insert(owner, bases);
        }
        self.owner_bases = owner_bases;
        let mut actor_content = HashMap::new();
        // A Ready handshake temporarily removes the connection grant, not the
        // simulation owner. Keep its identity alive until authority removes the
        // owner so the identity reserved in Welcome survives intervening ticks.
        let mut present: BTreeSet<_> = active_owners
            .iter()
            .filter(|&&owner| capture.position(ReplicationKey::Hero(owner)).is_some())
            .filter_map(|owner| self.ids.get(&Key::Owner(*owner)).copied())
            .filter(|id| self.present.contains(id))
            .collect();
        let reservations = capture.reserved_projectiles();
        if reservations.len() > self.max_identities {
            return Err(error("reserved projectile identity capacity"));
        }
        for (owner, id) in reservations {
            if owner == 0 || id == 0 {
                return Err(error("invalid reserved projectile"));
            }
            present.insert(self.identity(Key::Actor(ReplicationKey::Projectile(id)))?);
        }
        for (key, position, routes, visibility) in registrations {
            let radius = match key {
                Key::Actor(actor) => capture
                    .bounds(actor)
                    .map_or(0.0, |(_, radius)| radius as f64),
                _ => 0.0,
            };
            let id = self.identity(key)?;
            let unchanged = if let Key::Actor(ReplicationKey::CoverMarker(marker)) = key {
                self.capture.as_ref().is_some_and(|previous| {
                    previous.marker_state(marker) == capture.marker_state(marker)
                })
            } else if let Key::Actor(actor) = key {
                if let Some(value) = capture.project(actor, &all_sources) {
                    let previous = self.actor_content.get(&actor);
                    // Combat targets and dynamic blockers need fresh authority
                    // timestamps even while their positive DTO is unchanged.
                    // Otherwise renderer starvation would make mixed-time aim
                    // unavailable next to an actor that is still moving.
                    let combat =
                        matches!(actor, ReplicationKey::Hero(_) | ReplicationKey::Enemy(_))
                            || matches!(&value, PublicReplica::Cover(cover) if cover.present);
                    // Atomic owner dependencies require this publication's exact
                    // timestamp even when gameplay is paused or has ended.
                    let owner_dependency =
                        self.owner_bases.values().any(|bases| bases.contains(&id));
                    let unchanged = !owner_dependency
                        && previous.is_some_and(|old| {
                            old.value == value
                                && (!combat || tick.0.saturating_sub(old.sampled.0) < 3)
                        });
                    let sampled = if unchanged {
                        previous.unwrap().sampled
                    } else {
                        tick
                    };
                    actor_content.insert(actor, ActorContent { value, sampled });
                    unchanged
                } else {
                    false
                }
            } else {
                false
            };
            let version = if unchanged {
                self.graph
                    .entity(id)
                    .map_or(StateVersion(stamp.revision), |entry| entry.state_version)
            } else {
                StateVersion(stamp.revision)
            };
            present.insert(id);
            self.graph
                .apply(
                    tick,
                    Change::Upsert(EntityRegistration {
                        id,
                        bounds: Bounds {
                            center: [position[0] as f64, 0.0, position[1] as f64],
                            radius,
                        },
                        cull_radius: self.cull_radius,
                        routes,
                        visibility,
                        dependencies: if let Key::Owner(owner) = key {
                            std::iter::once(collision_entity)
                                .chain(self.owner_bases.get(&owner).into_iter().flatten().copied())
                                .collect()
                        } else {
                            BTreeSet::new()
                        },
                        state_version: version,
                        scene_revision: scene,
                        dormant: unchanged,
                    }),
                )
                .map_err(error)?;
        }
        for id in self
            .present
            .difference(&present)
            .copied()
            .collect::<Vec<_>>()
        {
            self.retired.insert(id);
            if self.graph.entity(id).is_some() {
                self.graph.apply(tick, Change::Remove(id)).map_err(error)?;
            }
            for peer in self.peers.values_mut() {
                if peer.previous.remove(&id) {
                    peer.scopes.destroy(id).map_err(error)?;
                }
            }
        }
        self.present = present;
        self.actor_content = actor_content;
        for (&connection, peer) in &self.peers {
            let owner = peer.player;
            let observers = if peer.ready == scene && active_owners.contains(&owner) {
                capture
                    .position(ReplicationKey::Hero(owner))
                    .map(|p| ObserverGrant {
                        position: [p[0] as f64, 0.0, p[1] as f64],
                        scene_revision: scene,
                    })
                    .into_iter()
                    .collect()
            } else {
                Vec::new()
            };
            let ready_scenes = if peer.ready == scene && active_owners.contains(&owner) {
                BTreeSet::from([scene])
            } else {
                BTreeSet::new()
            };
            let view = ConnectionView {
                observers,
                semantic_grants: BTreeSet::new(),
                ready_scenes,
            };
            self.graph
                .set_connection(
                    tick,
                    ConnectionId(connection),
                    view.clone(),
                    &PawnView {
                        owner: ConnectionId(connection),
                        expected: view,
                    },
                )
                .map_err(error)?;
        }
        let prepared = self
            .graph
            .prepare(ReplicationFrame(stamp.revision), tick, POLICY)
            .map_err(error)?;
        for (&connection, peer) in &mut self.peers {
            let owner = peer.player;
            let roots = if peer.ready == scene && active_owners.contains(&owner) {
                BTreeSet::from([self.ids[&Key::Global], self.ids[&Key::Owner(owner)]])
            } else {
                BTreeSet::new()
            };
            peer.eligible = Some(
                prepared
                    .gather(
                        ConnectionId(connection),
                        &peer.previous,
                        &roots,
                        &Policy {
                            connection: ConnectionId(connection),
                            owner: self.ids.get(&Key::Owner(owner)).copied(),
                            collision: collision_entity,
                            bases: self.owner_bases.get(&owner).cloned().unwrap_or_default(),
                        },
                    )
                    .map_err(error)?,
            );
        }
        self.canonical
            .begin_frame(prepared.frame(), tick, prepared.world_revision(), POLICY);
        self.capture = Some(capture);
        Ok(())
    }
    /// Projection occurs only for the prepared connection's opaque graph grants.
    /// Source IDs derive solely from independently eligible public heroes/enemies;
    /// dependency closure never reveals a hidden shooter to make an effect usable.
    #[cfg(test)]
    pub fn gather_peer(&mut self, owner: u64) -> Result<&EligibleSet, AdapterError> {
        let tick = self
            .capture
            .as_ref()
            .ok_or_else(|| error("not prepared"))?
            .global()
            .stamp
            .server_tick;
        self.gather_peer_with_frozen(
            owner,
            &BTreeSet::new(),
            ServerTick(tick),
            &Default::default(),
        )
    }
    pub fn gather_peer_with_frozen(
        &mut self,
        owner: u64,
        frozen: &BTreeSet<EntityId>,
        input_at: ServerTick,
        input_continuity: &engine_net::commands::InputContinuity<dreamwake_sim::DreamInput>,
    ) -> Result<&EligibleSet, AdapterError> {
        let capture = self.capture.as_ref().ok_or_else(|| error("not prepared"))?;
        if input_at.0 != capture.global().stamp.server_tick {
            return Err(error(
                "owner input continuity differs from committed capture tick",
            ));
        }
        let peer = self
            .peers
            .get_mut(&owner)
            .ok_or_else(|| error("unknown peer"))?;
        peer.sendable = false;
        let eligible = peer
            .eligible
            .as_ref()
            .ok_or_else(|| error("not gathered"))?;
        if !eligible.is_current(self.graph.accounting().world_revision, POLICY)
            || eligible.frame() != ReplicationFrame(capture.global().stamp.revision)
            || eligible.tick() != ServerTick(capture.global().stamp.server_tick)
        {
            return Err(error("stale prepared disclosure grant"));
        }
        peer.scopes
            .advance_barrier(eligible.world_revision(), POLICY)
            .map_err(error)?;
        let source_ids: BTreeSet<u64> = eligible
            .entries()
            .filter_map(|entry| match self.keys.get(&entry.entity()) {
                Some(Key::Actor(ReplicationKey::Hero(id) | ReplicationKey::Enemy(id))) => Some(*id),
                _ => None,
            })
            .collect();
        let sources = ApprovedSources::new(source_ids.iter().copied()).map_err(error)?;
        if source_ids != peer.sources {
            for &id in &peer.previous {
                let Some(Key::Actor(
                    key @ (ReplicationKey::Projectile(_)
                    | ReplicationKey::Wisp(_)
                    | ReplicationKey::Effect(_)),
                )) = self.keys.get(&id)
                else {
                    continue;
                };
                if capture.source_id(*key).is_some_and(|source| {
                    source_ids.contains(&source) != peer.sources.contains(&source)
                }) {
                    peer.scopes.exit(id).map_err(error)?;
                }
            }
        }
        let mut current = BTreeSet::new();
        for entry in eligible.entries() {
            let id = entry.entity();
            // The private owner checkpoint already supplies this actor. Keep
            // its graph grant for source authorization, but do not publish a
            // second public copy to the same player.
            if matches!(self.keys.get(&id), Some(Key::Actor(ReplicationKey::Hero(hero))) if *hero == peer.player)
            {
                continue;
            }
            if matches!(
                self.keys.get(&id),
                Some(Key::Actor(ReplicationKey::CoverMarker(_)))
            ) {
                continue;
            }
            // Source-audience changes revoke dependent scopes above. Cadence
            // can retain a current scope, but cannot defer its safe replacement.
            if frozen.contains(&id)
                && peer.previous.contains(&id)
                && peer.scopes.state(id).is_some()
            {
                current.insert(id);
                continue;
            }
            let payload = match self.keys.get(&id).ok_or_else(|| error("missing key"))? {
                Key::Global => ReplicaPayload::Global(capture.global().clone()),
                Key::Collision => ReplicaPayload::Collision(capture.collision()),
                Key::Owner(exact_owner) if *exact_owner == peer.player => {
                    let checkpoint = capture
                        .owner_checkpoint(peer.player, peer.ownership.0)
                        .and_then(|checkpoint| {
                            checkpoint.with_input_continuity(input_at, input_continuity.clone())
                        })
                        .map_err(error)?;
                    if checkpoint.collision_identity() != capture.collision().identity() {
                        return Err(error("owner checkpoint collision dependency mismatch"));
                    }
                    ReplicaPayload::Owner(checkpoint)
                }
                Key::Owner(_) => return Err(error("private owner grant mismatch")),
                Key::Actor(ReplicationKey::Effect(effect)) => {
                    // Preserve a collision-free opaque lifecycle identity without
                    // copying the hidden actor/action identity into a proxy.
                    let proxy = if id.index < (1 << 14) && id.generation < (1 << 18) {
                        (id.generation << 14) | id.index as u32
                    } else {
                        return Err(error("anonymous event identity exhausted"));
                    };
                    match capture.project_event(
                        *effect,
                        &sources,
                        ConnectionId(owner),
                        POLICY,
                        proxy,
                    ) {
                        Some(effect) => ReplicaPayload::Actor(PublicReplica::Effect(effect)),
                        None => continue,
                    }
                }
                Key::Actor(key) => match capture.project(*key, &sources) {
                    Some(actor) => ReplicaPayload::Actor(actor),
                    None => continue,
                },
            };
            // Only these positive DTOs are independent of source audiences and
            // recipient scope identities. This is an explicit game contract;
            // adding another variant requires proving the same equivalence.
            let canonical = matches!(
                self.keys.get(&id),
                Some(
                    Key::Global
                        | Key::Collision
                        | Key::Actor(
                            ReplicationKey::Hero(_)
                                | ReplicationKey::Enemy(_)
                                | ReplicationKey::Damage(_)
                                | ReplicationKey::Cover(_)
                                | ReplicationKey::Platform(_)
                        )
                )
            );
            let bytes = if canonical {
                self.canonical
                    .encode(eligible, id, || {
                        encode_replica_with_compression(&payload, self.compression)
                    })
                    .map_err(error)?
            } else {
                encode_replica_with_compression(&payload, self.compression).map_err(error)?
            };
            peer.scopes
                .offer(
                    eligible,
                    id,
                    self.graph.entity(id).is_some_and(|entry| entry.dormant),
                    |_| bytes,
                )
                .map_err(error)?;
            current.insert(id);
        }
        // Adornments never add a visibility grant to their carrier. Both roots
        // must already be independently eligible. Scope references are produced
        // after the parent's current full-state identity has been selected.
        for entry in eligible.entries() {
            let id = entry.entity();
            let Some(Key::Actor(ReplicationKey::CoverMarker(marker))) = self.keys.get(&id) else {
                continue;
            };
            let Some(parent_id) = capture
                .marker_parent(*marker)
                .and_then(|cover| {
                    self.ids
                        .get(&Key::Actor(ReplicationKey::Cover(cover)))
                        .copied()
                })
                .filter(|parent| current.contains(parent) && eligible.get(*parent).is_some())
            else {
                continue;
            };
            let Some(parent) = peer.scopes.state(parent_id).map(|state| state.scope) else {
                continue;
            };
            let Some(marker) = capture.project_cover_marker(*marker, parent) else {
                continue;
            };
            let parent_changed = peer.scopes.state(id)
                .and_then(|state| decode_replica(&state.payload, None).ok())
                .is_some_and(|payload| matches!(payload, ReplicaPayload::Actor(PublicReplica::CoverMarker(previous)) if previous.parent != parent));
            if parent_changed {
                peer.scopes.exit(id).map_err(error)?;
            }
            let bytes = encode_replica_with_compression(
                &ReplicaPayload::Actor(PublicReplica::CoverMarker(marker)),
                self.compression,
            )
            .map_err(error)?;
            peer.scopes
                .offer(eligible, id, true, |_| bytes)
                .map_err(error)?;
            current.insert(id);
        }
        for &id in peer.previous.difference(&current) {
            peer.scopes.exit(id).map_err(error)?;
        }
        peer.previous = current;
        peer.sources = source_ids;
        peer.sendable = true;
        Ok(eligible)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dreamwake_protocol::replication::encode_replica;
    use dreamwake_sim::replication::ReplicationStamp;
    use dreamwake_sim::{DreamInput, DreamSimulation};
    use engine_net::replication::DeliveryPhase;
    fn capture(sim: &DreamSimulation, revision: u64) -> CommittedReplication {
        sim.capture_replication(ReplicationStamp {
            match_epoch: 7,
            server_tick: revision,
            gameplay_tick: sim.snapshot().tick,
            scene_revision: 1,
            revision,
        })
        .unwrap()
    }
    fn adapter() -> DreamReplication {
        DreamReplication::new(
            7,
            12.0,
            Limits {
                prefetch: 0.0,
                leave_margin: 0.0,
                ..Limits::default()
            },
        )
        .unwrap()
    }
    fn party() -> DreamSimulation {
        let mut sim = DreamSimulation::new(42, false);
        assert!(sim.add_player(2));
        sim
    }
    fn join(rep: &mut DreamReplication, id: u64) {
        rep.set_peer(id, ConnectionEpoch(id), SceneRevision(1))
            .unwrap();
    }
    #[test]
    fn cover_region_reentry_transmits_current_absence_and_marker_parent_scope() {
        use engine_net::replication::ClientScopes;
        let mut sim = DreamSimulation::new(42, false);
        sim.continue_run();
        let cover_id = sim.snapshot().covers[0].id;
        let mut rep = adapter();
        join(&mut rep, 1);
        rep.prepare(capture(&sim, 1), &[1]).unwrap();
        rep.gather_peer(1).unwrap();
        let cover_entity = rep.actor_entity(ReplicationKey::Cover(cover_id)).unwrap();
        let cover = rep
            .scopes_mut(1)
            .unwrap()
            .state(cover_entity)
            .unwrap()
            .clone();
        let marker_entity = rep
            .keys
            .iter()
            .find_map(|(&id, key)| {
                let Key::Actor(ReplicationKey::CoverMarker(marker_id)) = key else {
                    return None;
                };
                (rep.capture.as_ref().unwrap().marker_parent(*marker_id) == Some(cover_id))
                    .then_some(id)
            })
            .unwrap();
        let marker = rep
            .scopes_mut(1)
            .unwrap()
            .state(marker_entity)
            .unwrap()
            .clone();
        let ReplicaPayload::Actor(PublicReplica::CoverMarker(marker_view)) =
            decode_replica(&marker.payload, None).unwrap()
        else {
            panic!("marker root")
        };
        assert_eq!(marker_view.parent, cover.scope);
        let mut client = ClientScopes::new(ConnectionEpoch(1), ScopeLimits::default()).unwrap();
        client
            .apply_full(marker.clone(), |bytes| decode_replica(bytes, None).is_ok())
            .unwrap();
        assert!(matches!(
            marker_view
                .attachment()
                .resolve(marker.scope, &client)
                .unwrap(),
            engine_net::replication::attachments::AttachmentResolution::Staged
        ));
        client
            .apply_full(cover.clone(), |bytes| decode_replica(bytes, None).is_ok())
            .unwrap();
        assert!(matches!(
            marker_view
                .attachment()
                .resolve(marker.scope, &client)
                .unwrap(),
            engine_net::replication::attachments::AttachmentResolution::Parent { .. }
        ));

        rep.prepare(capture(&sim, 2), &[1]).unwrap();
        rep.gather_peer(1).unwrap();
        assert_eq!(
            rep.scopes_mut(1).unwrap().state(marker_entity),
            Some(&marker),
            "unchanged marker must retain its existing publication"
        );

        rep.set_peer(1, ConnectionEpoch(1), SceneRevision(2))
            .unwrap();
        rep.prepare(capture(&sim, 3), &[1]).unwrap();
        rep.gather_peer(1).unwrap();
        let exits = rep
            .scopes_mut(1)
            .unwrap()
            .pending_exits()
            .collect::<Vec<_>>();
        for exit in exits {
            client.apply_exit(exit).unwrap();
        }
        assert!(sim.remove_cover(cover_id).unwrap());
        sim.step(DreamInput::default());
        rep.prepare(capture(&sim, 4), &[1]).unwrap();
        rep.gather_peer(1).unwrap();
        assert!(rep.scopes_mut(1).unwrap().state(cover_entity).is_none());

        rep.set_peer(1, ConnectionEpoch(1), SceneRevision(1))
            .unwrap();
        rep.prepare(capture(&sim, 5), &[1]).unwrap();
        rep.gather_peer(1).unwrap();
        let absent = rep
            .scopes_mut(1)
            .unwrap()
            .state(cover_entity)
            .unwrap()
            .clone();
        assert!(absent.scope.scope > cover.scope.scope);
        let ReplicaPayload::Actor(PublicReplica::Cover(absent_view)) =
            decode_replica(&absent.payload, None).unwrap()
        else {
            panic!("cover region")
        };
        assert!(!absent_view.present);
        client
            .apply_full(absent, |bytes| decode_replica(bytes, None).is_ok())
            .unwrap();
        assert!(client.apply_full(cover, |_| true).is_err());
        assert!(rep.scopes_mut(1).unwrap().state(marker_entity).is_none());
    }
    #[test]
    fn visible_enemy_pulse_keeps_attack_geometry_and_fences_source_visibility_changes() {
        use engine_net::replication::ClientScopes;

        let mut sim = DreamSimulation::new(42, false);
        sim.continue_run();
        let enemy = sim
            .snapshot()
            .enemies
            .into_iter()
            .find(|enemy| enemy.position == [101.0, 69.0])
            .unwrap();
        let target = [103.0, 69.0];
        place_player(&mut sim, 1, target);
        let mut pulse = (0..60)
            .find_map(|_| {
                sim.step(DreamInput::default());
                sim.snapshot()
                    .presentations
                    .into_iter()
                    .find(|effect| effect.id.owner == enemy.id && effect.id.slot == 0xc200)
            })
            .expect("ambient melee enemy commits and resolves its attack");
        assert!(enemy.id >= 1 << 63);
        let target = sim
            .snapshot()
            .enemies
            .into_iter()
            .find(|current| current.id == enemy.id)
            .unwrap()
            .target;
        assert_eq!(pulse.pos, target);
        assert_eq!(pulse.radius, 1.9);
        assert_eq!(pulse.duration_ticks, 22);
        sim.step(DreamInput {
            attack: true,
            aim: [1.0, 0.0],
            ..Default::default()
        });
        let snapshot = sim.snapshot();
        pulse = *snapshot
            .presentations
            .iter()
            .find(|effect| effect.id == pulse.id)
            .unwrap();
        let friendly = snapshot
            .presentations
            .iter()
            .find(|effect| effect.id.owner == 1 && effect.id.slot == 0x8000)
            .unwrap()
            .id;

        let mut rep = adapter();
        join(&mut rep, 1);
        rep.prepare(capture(&sim, 1), &[1]).unwrap();
        let source = rep.actor_entity(ReplicationKey::Enemy(enemy.id)).unwrap();
        let event = rep.actor_entity(ReplicationKey::Effect(pulse.id)).unwrap();
        let friendly_event = rep.actor_entity(ReplicationKey::Effect(friendly)).unwrap();
        assert!(rep.gather_peer(1).unwrap().get(source).is_some());
        let visible = rep.scopes_mut(1).unwrap().state(event).unwrap().clone();
        let friendly_scope = rep
            .scopes_mut(1)
            .unwrap()
            .state(friendly_event)
            .unwrap()
            .scope;
        let ReplicaPayload::Actor(PublicReplica::Effect(effect)) =
            decode_replica(&visible.payload, None).unwrap()
        else {
            panic!("enemy attack effect")
        };
        assert_eq!(effect.id.owner, enemy.id);
        assert_eq!(effect.id.action_seq, pulse.id.action_seq);
        assert_eq!(effect.id.slot, pulse.id.slot);
        assert_eq!(effect.kind, pulse.kind);
        assert_eq!(effect.position, target);
        assert_eq!(effect.radius, pulse.radius);
        assert_eq!(effect.age_ticks, pulse.age_ticks);
        assert_eq!(effect.duration_ticks, pulse.duration_ticks);
        assert_eq!(
            rep.actor_content[&ReplicationKey::Effect(pulse.id)].value,
            PublicReplica::Effect(effect)
        );
        let mut client = ClientScopes::new(ConnectionEpoch(1), ScopeLimits::default()).unwrap();
        client.apply_full(visible.clone(), |_| true).unwrap();
        let mut previous_state = visible;

        // The impact remains at its committed target while its source leaves
        // interest. Even a cadence-frozen event must replace its disclosed scope.
        for (revision, position, source_visible) in
            [(2, [70.0, 69.0], false), (3, enemy.position, true)]
        {
            let previous = sim
                .snapshot()
                .enemies
                .into_iter()
                .find(|current| current.id == enemy.id)
                .unwrap()
                .position;
            let world = sim.world_mut();
            let mut query = world.query::<&mut engine_core::MotorState>();
            query
                .iter_mut(world)
                .find(|motor| motor.position == previous)
                .unwrap()
                .position = position;
            rep.prepare(capture(&sim, revision), &[1]).unwrap();
            let eligible = rep
                .gather_peer_with_frozen(
                    1,
                    &BTreeSet::from([event, friendly_event]),
                    ServerTick(revision),
                    &Default::default(),
                )
                .unwrap();
            assert_eq!(eligible.get(source).is_some(), source_visible);
            assert_eq!(
                rep.scopes_mut(1)
                    .unwrap()
                    .state(friendly_event)
                    .unwrap()
                    .scope,
                friendly_scope,
                "an unrelated source change must not churn the player's effect scope"
            );
            let state = rep.scopes_mut(1).unwrap().state(event).unwrap().clone();
            assert!(state.scope.scope > previous_state.scope.scope);
            let ReplicaPayload::Actor(PublicReplica::Effect(current)) =
                decode_replica(&state.payload, None).unwrap()
            else {
                panic!("source audience projection")
            };
            if source_visible {
                assert_eq!(current, effect);
            } else {
                assert_eq!(current.id.owner, 0);
                assert_eq!(current.id.slot, u16::MAX);
                assert_eq!(
                    current.position,
                    target.map(|v| ((v / 8.0).floor() + 0.5) * 8.0)
                );
                assert_eq!(current.radius, 8.0);
                assert!(rep.scopes_mut(1).unwrap().state(source).is_none());
            }
            client.apply_full(state.clone(), |_| true).unwrap();
            assert!(client.apply_full(previous_state, |_| true).is_err());
            previous_state = state;
        }
    }
    #[test]
    fn hidden_source_pulse_uses_independent_coarse_event_audience() {
        let mut sim = party();
        sim.continue_run_for(1);
        sim.continue_run_for(2);
        for _ in 0..180 {
            sim.step_multiplayer(&[
                (
                    1,
                    DreamInput {
                        movement: [-1.0, 0.0],
                        ..Default::default()
                    },
                ),
                (
                    2,
                    DreamInput {
                        movement: [1.0, 0.0],
                        ..Default::default()
                    },
                ),
            ]);
        }
        let position = capture(&sim, 1).position(ReplicationKey::Hero(1)).unwrap();
        sim.step_multiplayer(&[(
            2,
            DreamInput {
                aim: [-1.0, 0.0],
                casts: [false, true, false, false],
                action_sequences: [0, 0, 909_991, 0, 0],
                ..Default::default()
            },
        )]);
        let id = {
            let world = sim.world_mut();
            let mut query = world.query::<&mut engine_core::GraphicsInstance>();
            let mut effect = query
                .iter_mut(world)
                .find(|effect| {
                    effect.id.owner == 2
                        && matches!(effect.kind, engine_core::GraphicsKind::RadialPulse)
                })
                .expect("cast pulse");
            effect.pos = position;
            effect.id
        };
        let mut rep = adapter();
        join(&mut rep, 1);
        rep.prepare(capture(&sim, 2), &[1, 2]).unwrap();
        let source = rep.actor_entity(ReplicationKey::Hero(2)).unwrap();
        let event = rep.actor_entity(ReplicationKey::Effect(id)).unwrap();
        assert!(rep.gather_peer(1).unwrap().get(source).is_none());
        let state = rep.scopes_mut(1).unwrap().state(event).unwrap();
        let ReplicaPayload::Actor(PublicReplica::Effect(proxy)) =
            decode_replica(&state.payload, None).unwrap()
        else {
            panic!("event proxy")
        };
        assert_eq!(proxy.id.owner, 0);
        assert!(proxy.id.scope.is_none());
        assert_ne!(proxy.id.action_seq, id.action_seq);
        assert_eq!(
            proxy.position,
            position.map(|v| ((v / 8.0).floor() + 0.5) * 8.0)
        );
        assert_eq!(proxy.radius, 8.0);
        assert_ne!(proxy.position, position);
        assert!(rep.scopes_mut(1).unwrap().state(source).is_none());
    }
    #[test]
    fn delayed_echo_keeps_reserved_identity_until_spawn_or_owner_departure() {
        for cancel_before_spawn in [false, true] {
            let mut sim = DreamSimulation::new(42, false);
            sim.continue_run();
            {
                let world = sim.world_mut();
                for mut loadout in
                    world
                        .query::<&mut engine_core::Loadout<
                            dreamwake_sim::MemoryKind,
                            dreamwake_sim::EssenceKind,
                        >>()
                        .iter_mut(world)
                {
                    loadout.0[1].modifier = Some(dreamwake_sim::EssenceKind::Echo);
                }
            }
            let key = dreamwake_sim::combat::RayActionKey {
                match_epoch: 7,
                connection_epoch: 1,
                command_stream: 1,
                ownership_epoch: 1,
                actor: 1,
                actor_generation: 1,
                command_sequence: 1,
                action_slot: 0,
            };
            sim.step_multiplayer_with_actions(
                &[],
                &[dreamwake_sim::combat::CombatAction::Cast {
                    key,
                    execution_server_tick: 1,
                    slot: 1,
                    aim: [1.0, 0.0],
                }],
            )
            .unwrap();
            let future = sim
                .starfall_bindings(key)
                .into_iter()
                .find(|(ordinal, _)| *ordinal == 3)
                .unwrap()
                .1;
            let mut rep = adapter();
            join(&mut rep, 1);
            let reserved = rep.reserve_projectile_identity(future).unwrap();
            let first = capture(&sim, 1);
            assert!(first.reserved_projectiles().contains(&(1, future)));
            rep.prepare(first, &[1]).unwrap();
            assert!(rep.present.contains(&reserved));
            assert!(
                rep.graph.entity(reserved).is_none(),
                "reservation cannot disclose future actor"
            );
            if !cancel_before_spawn {
                let mut spawned = false;
                for revision in 2..=40 {
                    sim.step(DreamInput::default());
                    let cap = capture(&sim, revision);
                    let exists = cap.keys().contains(&ReplicationKey::Projectile(future));
                    rep.prepare(cap, &[1]).unwrap();
                    assert!(rep.present.contains(&reserved));
                    assert_eq!(
                        rep.actor_entity(ReplicationKey::Projectile(future)),
                        Some(reserved)
                    );
                    if exists {
                        spawned = true;
                        assert!(rep.graph.entity(reserved).is_some());
                        break;
                    }
                }
                assert!(
                    spawned,
                    "Echo must use its originally accepted authority binding"
                );
            }
            assert!(sim.set_player_active(1, false));
            rep.prepare(capture(&sim, 50), &[1]).unwrap();
            assert!(rep.retired.contains(&reserved));
            assert!(rep.graph.entity(reserved).is_none());
            assert!(!rep.present.contains(&reserved));
        }
    }

    #[test]
    fn canceled_unpublished_projectile_reservation_retires_its_generation() {
        let sim = party();
        let mut rep = adapter();
        join(&mut rep, 1);
        let reserved = rep.reserve_projectile_identity(900).unwrap();
        assert!(rep.present.contains(&reserved));
        assert!(rep.graph.entity(reserved).is_none());
        // Absence from the authoritative pending/live set cancels liveness.
        rep.prepare(capture(&sim, 1), &[1]).unwrap();
        assert!(rep.retired.contains(&reserved));
        assert!(rep.graph.entity(reserved).is_none());
        let newer = rep.reserve_projectile_identity(900).unwrap();
        assert_eq!(reserved.index, newer.index);
        assert!(newer.generation > reserved.generation);
    }

    #[test]
    fn unchanged_platform_dependency_advances_with_atomic_owner_checkpoint() {
        let mut sim = DreamSimulation::new(42, false);
        sim.continue_run_for(1);
        {
            let world = sim.world_mut();
            let mut query = world.query::<&mut engine_core::KinematicState>();
            *query.single_mut(world).unwrap() =
                engine_core::KinematicState::new([1.0, 0.25, -10.0], 1);
        }
        sim.step(DreamInput::default());
        let mut rep = adapter();
        join(&mut rep, 1);
        rep.prepare(capture(&sim, 1), &[1]).unwrap();
        rep.gather_peer(1).unwrap();
        let base = *rep.owner_bases[&1].first().unwrap();
        let first = rep.scopes_mut(1).unwrap().pending(base).unwrap().clone();
        // Same simulation state, new authority publication (pause/victory).
        rep.prepare(capture(&sim, 2), &[1]).unwrap();
        rep.gather_peer(1).unwrap();
        let second = rep.scopes_mut(1).unwrap().pending(base).unwrap();
        assert_eq!(first.payload, second.payload);
        assert_eq!(second.end_tick, ServerTick(2));
        assert!(second.version > first.version);
    }

    #[test]
    fn platform_dependency_survives_offscreen_observer_but_never_a_denied_grant() {
        let mut sim = DreamSimulation::new(42, false);
        sim.continue_run_for(1);
        {
            let world = sim.world_mut();
            let mut query = world.query::<&mut engine_core::KinematicState>();
            let mut motion = query.single_mut(world).unwrap();
            *motion = engine_core::KinematicState::new([1.0, 0.25, -10.0], 1);
        }
        sim.step(DreamInput::default());
        let captured = capture(&sim, 1);
        assert_eq!(captured.required_bases(1).len(), 1);
        let mut rep = adapter();
        join(&mut rep, 1);
        rep.prepare(captured, &[1]).unwrap();
        let members = rep.owner_group_entities(1).unwrap();
        assert_eq!(members.len(), 4);
        let base = *rep.owner_bases[&1].first().unwrap();
        let owner = rep.owner_entity(1).unwrap();
        let global = rep.global_entity().unwrap();
        let collision = rep.collision_entity().unwrap();
        // The platform is outside the observer's spatial audience; the exact
        // authorized owner dependency still admits its complete immutable DTO.
        let view = ConnectionView {
            observers: vec![ObserverGrant {
                position: [1000.0, 0.0, 1000.0],
                scene_revision: SceneRevision(1),
            }],
            semantic_grants: BTreeSet::new(),
            ready_scenes: BTreeSet::from([SceneRevision(1)]),
        };
        rep.graph
            .set_connection(
                ServerTick(1),
                ConnectionId(1),
                view.clone(),
                &PawnView {
                    owner: ConnectionId(1),
                    expected: view,
                },
            )
            .unwrap();
        let policy = Policy {
            connection: ConnectionId(1),
            owner: Some(owner),
            collision,
            bases: BTreeSet::from([base]),
        };
        {
            let prepared = rep
                .graph
                .prepare(ReplicationFrame(2), ServerTick(1), POLICY)
                .unwrap();
            let eligible = prepared
                .gather(
                    ConnectionId(1),
                    &BTreeSet::new(),
                    &BTreeSet::from([global, owner]),
                    &policy,
                )
                .unwrap();
            assert!(eligible.get(base).is_some());
            assert!(matches!(
                eligible.prediction(),
                PredictionAdmission::Admitted { .. }
            ));
        }
        let mut hidden = rep.graph.entity(base).unwrap().clone();
        hidden.visibility = Visibility::Owner(ConnectionId(99));
        rep.graph
            .apply(ServerTick(1), Change::Upsert(hidden))
            .unwrap();
        let prepared = rep
            .graph
            .prepare(ReplicationFrame(3), ServerTick(1), POLICY)
            .unwrap();
        let denied = prepared
            .gather(
                ConnectionId(1),
                &BTreeSet::new(),
                &BTreeSet::from([global, owner]),
                &policy,
            )
            .unwrap();
        assert!(denied.get(base).is_none());
        assert!(matches!(
            denied.prediction(),
            PredictionAdmission::Denied(_)
        ));
    }

    #[test]
    fn owner_group_requires_collision_scope_and_exact_allowlisted_edge() {
        let sim = party();
        let mut rep = adapter();
        join(&mut rep, 1);
        join(&mut rep, 2);
        rep.prepare(capture(&sim, 1), &[1, 2]).unwrap();
        let global = rep.global_entity().unwrap();
        let owner = rep.owner_entity(1).unwrap();
        let other_owner = rep.owner_entity(2).unwrap();
        let collision = rep.collision_entity().unwrap();
        let eligible = rep.gather_peer(1).unwrap().clone();
        assert_eq!(
            eligible.prediction(),
            &PredictionAdmission::Admitted {
                members: BTreeSet::from([global, owner, collision]),
            }
        );
        assert!(eligible.get(other_owner).is_none());
        let scopes = rep.scopes_mut(1).unwrap();
        for entity in [global, owner, collision] {
            assert_eq!(scopes.pending(entity).unwrap().end_tick, ServerTick(1));
        }
        let ReplicaPayload::Collision(manifest) = dreamwake_protocol::replication::decode_replica(
            &scopes.pending(collision).unwrap().payload,
            None,
        )
        .unwrap() else {
            panic!("collision payload")
        };
        assert_eq!(
            manifest.identity(),
            dreamwake_protocol::live::ContentIdentity::current(SceneRevision(1)).scene
        );
        // An accidental private dependency cannot reuse the collision allowlist.
        let mut registration = rep.graph.entity(owner).unwrap().clone();
        registration.dependencies.insert(other_owner);
        rep.graph
            .apply(ServerTick(1), Change::Upsert(registration))
            .unwrap();
        let prepared = rep
            .graph
            .prepare(ReplicationFrame(2), ServerTick(1), POLICY)
            .unwrap();
        let rejected = prepared
            .gather(
                ConnectionId(1),
                &BTreeSet::new(),
                &BTreeSet::from([global, owner]),
                &Policy {
                    connection: ConnectionId(1),
                    owner: Some(owner),
                    collision,
                    bases: BTreeSet::new(),
                },
            )
            .unwrap();
        assert!(matches!(
            rejected.prediction(),
            PredictionAdmission::Denied(DependencyError::Denied)
        ));
        assert!(rejected.get(other_owner).is_none());
    }

    #[test]
    fn missing_collision_dependency_never_admits_partial_owner_prediction() {
        let sim = party();
        let mut rep = adapter();
        join(&mut rep, 1);
        rep.prepare(capture(&sim, 1), &[1]).unwrap();
        let collision = rep.collision_entity().unwrap();
        let global = rep.global_entity().unwrap();
        let owner = rep.owner_entity(1).unwrap();
        rep.graph
            .apply(ServerTick(1), Change::Remove(collision))
            .unwrap();
        let prepared = rep
            .graph
            .prepare(ReplicationFrame(2), ServerTick(1), POLICY)
            .unwrap();
        let eligible = prepared
            .gather(
                ConnectionId(1),
                &BTreeSet::new(),
                &BTreeSet::from([global, owner]),
                &Policy {
                    connection: ConnectionId(1),
                    owner: Some(owner),
                    collision,
                    bases: BTreeSet::new(),
                },
            )
            .unwrap();
        assert_eq!(
            eligible.prediction(),
            &PredictionAdmission::Denied(DependencyError::Missing)
        );
        assert!(eligible.get(collision).is_none());
        assert!(
            engine_net::scheduling::UnitRequest::prediction_group(
                &eligible,
                dreamwake_protocol::live::OWNER_GROUP,
                100,
                engine_net::scheduling::Cadence {
                    period_ticks: 1,
                    maximum_age_ticks: 6,
                    priority: 255,
                    critical: true
                }
            )
            .is_err()
        );
    }

    #[test]
    fn waiting_ready_preserves_owner_identity_but_departure_retires_it() {
        let sim = party();
        let mut rep = adapter();
        join(&mut rep, 1);
        rep.prepare(capture(&sim, 1), &[1, 2]).unwrap();
        rep.gather_peer(1).unwrap();
        let old = rep.owner_entity(1).unwrap();
        rep.disconnect(1, ServerTick(2)).unwrap();
        let (_, advertised, _) = rep.reserve_owner(1).unwrap();
        assert_eq!(advertised, old);
        rep.prepare(capture(&sim, 2), &[1, 2]).unwrap();
        assert!(rep.gather_peer(1).is_err());
        rep.set_peer(1, ConnectionEpoch(2), SceneRevision(1))
            .unwrap();
        rep.prepare(capture(&sim, 3), &[1, 2]).unwrap();
        assert_eq!(rep.owner_entity(1), Some(advertised));
        rep.gather_peer(1).unwrap();
        assert!(rep.scopes_mut(1).unwrap().pending(advertised).is_some());
        rep.disconnect(1, ServerTick(4)).unwrap();
        rep.prepare(capture(&sim, 4), &[2]).unwrap();
        let (_, replacement, _) = rep.reserve_owner(1).unwrap();
        assert_eq!(replacement.index, old.index);
        assert!(replacement.generation > old.generation);
    }
    #[test]
    fn stable_namespaced_bounded_identity_and_match_fence() {
        let mut rep = adapter();
        let hero = rep.identity(Key::Actor(ReplicationKey::Hero(1))).unwrap();
        assert_eq!(
            hero,
            rep.identity(Key::Actor(ReplicationKey::Hero(1))).unwrap()
        );
        assert_ne!(
            hero,
            rep.identity(Key::Actor(ReplicationKey::Projectile(1)))
                .unwrap()
        );
        assert_ne!(hero, rep.identity(Key::Owner(1)).unwrap());
        rep.max_identities = rep.ids.len();
        assert!(rep.identity(Key::Actor(ReplicationKey::Wisp(1))).is_err());
        let wrong = DreamReplication::new(8, 12.0, Limits::default())
            .unwrap()
            .identity(Key::Actor(ReplicationKey::Hero(1)))
            .unwrap();
        assert_ne!(hero, wrong);
    }
    #[test]
    fn retired_actor_slots_recycle_with_generation_fences_under_lifetime_churn() {
        let mut rep = adapter();
        let global = rep.identity(Key::Global).unwrap();
        let mut previous = rep
            .identity(Key::Actor(ReplicationKey::Projectile(1)))
            .unwrap();
        rep.max_identities = 2;
        for key in 2..=20_000 {
            rep.retired.insert(previous);
            let next = rep
                .identity(Key::Actor(ReplicationKey::Projectile(key)))
                .unwrap();
            assert_eq!(next.index, previous.index);
            assert!(next.generation > previous.generation);
            assert!(!rep.keys.contains_key(&previous));
            assert_eq!(rep.ids.len(), 2);
            assert_eq!(rep.global_entity(), Some(global));
            previous = next;
        }
    }

    #[test]
    fn public_and_owner_grants_are_separate_and_prepare_is_shared() {
        let sim = party();
        let mut rep = adapter();
        join(&mut rep, 1);
        join(&mut rep, 2);
        rep.prepare(capture(&sim, 1), &[1, 2]).unwrap();
        let private_two = rep.owner_entity(2).unwrap();
        let public_two = rep.actor_entity(ReplicationKey::Hero(2)).unwrap();
        let one = rep.gather_peer(1).unwrap();
        assert!(one.get(private_two).is_none());
        assert!(one.get(public_two).is_some());
        let bytes = &rep
            .scopes_mut(1)
            .unwrap()
            .pending(public_two)
            .unwrap()
            .payload;
        let dreamwake_protocol::replication::ReplicaPayload::Actor(actor) =
            dreamwake_protocol::replication::decode_replica(bytes, None).unwrap()
        else {
            panic!("public actor representation");
        };
        assert!(
            matches!(actor, dreamwake_sim::replication::PublicReplica::Hero(hero) if hero.id == 2)
        );
        rep.gather_peer(2).unwrap();
        assert!(rep.scopes_mut(2).unwrap().pending(private_two).is_some());
        assert_eq!(rep.accounting().prepare_calls, 1);
        assert_eq!(rep.accounting().observers, 2);
    }
    #[test]
    fn unready_scene_revokes_and_reentry_uses_new_scope() {
        let sim = party();
        let mut rep = adapter();
        join(&mut rep, 1);
        rep.prepare(capture(&sim, 1), &[1, 2]).unwrap();
        rep.gather_peer(1).unwrap();
        let actor = rep.actor_entity(ReplicationKey::Hero(2)).unwrap();
        let old = rep.scopes_mut(1).unwrap().pending(actor).unwrap().scope;
        rep.set_peer(1, ConnectionEpoch(1), SceneRevision(2))
            .unwrap();
        rep.prepare(capture(&sim, 2), &[1, 2]).unwrap();
        assert_eq!(rep.gather_peer(1).unwrap().entries().len(), 0);
        assert!(rep.scopes_mut(1).unwrap().pending(actor).is_none());
        assert_eq!(
            rep.scopes_mut(1).unwrap().phase(actor),
            Some(DeliveryPhase::Leaving)
        );
        rep.set_peer(1, ConnectionEpoch(1), SceneRevision(1))
            .unwrap();
        rep.prepare(capture(&sim, 3), &[1, 2]).unwrap();
        rep.gather_peer(1).unwrap();
        assert!(
            rep.scopes_mut(1)
                .unwrap()
                .pending(actor)
                .unwrap()
                .scope
                .scope
                > old.scope
        );
    }
    fn place_player(sim: &mut DreamSimulation, player: u64, position: [f32; 2]) {
        let previous = sim.snapshot_for(player).hero.position;
        let world = sim.world_mut();
        let mut query = world.query::<&mut engine_core::KinematicState>();
        let mut matches = query
            .iter_mut(world)
            .filter(|motion| [motion.position[0], motion.position[2]] == previous);
        let mut motion = matches.next().expect("unique hero motion");
        *motion = engine_core::KinematicState::new([position[0], 0.0, position[1]], 1);
        assert!(matches.next().is_none());
    }

    #[test]
    fn ambient_enemy_aggro_never_bypasses_culling_and_reentry_uses_current_scope() {
        let mut sim = party();
        sim.continue_run_for(1);
        sim.continue_run_for(2);
        let enemy = sim
            .snapshot()
            .enemies
            .iter()
            .find(|enemy| enemy.position == [101.0, 69.0])
            .unwrap()
            .clone();
        place_player(&mut sim, 1, [109.0, 69.0]);
        place_player(&mut sim, 2, [-100.0, -100.0]);
        sim.step_multiplayer(&[]);
        assert!(sim.player_is_active(1) && sim.player_is_active(2));
        assert!(sim.snapshot_for(1).hero.hp > 0.0 && sim.snapshot_for(2).hero.hp > 0.0);
        let mut rep = adapter();
        join(&mut rep, 1);
        join(&mut rep, 2);
        rep.prepare(capture(&sim, 1), &[1, 2]).unwrap();
        rep.gather_peer(1).unwrap();
        let entity = rep.actor_entity(ReplicationKey::Enemy(enemy.id)).unwrap();
        let initial = rep.scopes_mut(1).unwrap().pending(entity).unwrap().clone();
        rep.scopes_mut(1)
            .unwrap()
            .mark_sent(initial.receipt())
            .unwrap();
        rep.scopes_mut(1)
            .unwrap()
            .acknowledge(initial.receipt())
            .unwrap();

        // A different active owner wakes this enemy while the original observer
        // is outside disclosure range. AI state changes grant no audience.
        place_player(&mut sim, 1, [-100.0, 100.0]);
        place_player(&mut sim, 2, [109.0, 69.0]);
        let before = sim
            .snapshot()
            .enemies
            .iter()
            .find(|v| v.id == enemy.id)
            .unwrap()
            .position;
        for revision in 2..=61 {
            sim.step_multiplayer(&[]);
            rep.prepare(capture(&sim, revision), &[1, 2]).unwrap();
            let hidden = rep.gather_peer(1).unwrap();
            assert!(hidden.get(entity).is_none());
            assert!(rep.scopes_mut(1).unwrap().pending(entity).is_none());
            assert!(rep.scopes_mut(1).unwrap().state(entity).is_none());
            assert!(rep.gather_peer(2).unwrap().get(entity).is_some());
        }
        let current = sim
            .snapshot()
            .enemies
            .iter()
            .find(|v| v.id == enemy.id)
            .unwrap()
            .clone();
        assert_ne!(
            current.position, before,
            "another owner must actually wake the enemy"
        );
        assert!(
            rep.scopes_mut(1)
                .unwrap()
                .pending_exits()
                .any(|exit| exit.scope == initial.scope)
        );
        place_player(
            &mut sim,
            1,
            [current.position[0] - 8.0, current.position[1]],
        );
        sim.step_multiplayer(&[]);
        rep.prepare(capture(&sim, 62), &[1, 2]).unwrap();
        rep.gather_peer(1).unwrap();
        let entered = rep.scopes_mut(1).unwrap().pending(entity).unwrap();
        assert!(entered.scope.scope > initial.scope.scope);
        assert_eq!(entered.end_tick, ServerTick(62));
        let ReplicaPayload::Actor(PublicReplica::Enemy(view)) =
            decode_replica(&entered.payload, None).unwrap()
        else {
            panic!("positive enemy presentation")
        };
        assert_eq!(
            view.position,
            sim.snapshot()
                .enemies
                .iter()
                .find(|v| v.id == enemy.id)
                .unwrap()
                .position
        );
        assert_ne!(entered.receipt(), initial.receipt());
        let current_receipt = entered.receipt();
        assert!(
            rep.scopes_mut(1)
                .unwrap()
                .acknowledge(initial.receipt())
                .is_err()
        );
        assert_eq!(
            rep.scopes_mut(1)
                .unwrap()
                .pending(entity)
                .unwrap()
                .receipt(),
            current_receipt
        );
    }

    #[test]
    fn idle_enemy_inside_disclosure_still_refreshes_combat_authority_time() {
        let mut sim = DreamSimulation::new(42, false);
        sim.continue_run_for(1);
        let enemy = sim
            .snapshot()
            .enemies
            .iter()
            .find(|enemy| enemy.position == [101.0, 69.0])
            .unwrap()
            .clone();
        place_player(&mut sim, 1, [71.0, 69.0]);
        assert!(30.0 > dreamwake_sim::ENEMY_AGGRO_RANGE);
        sim.step(DreamInput::default());
        let mut rep = DreamReplication::new(
            7,
            40.0,
            Limits {
                prefetch: 0.0,
                leave_margin: 0.0,
                ..Limits::default()
            },
        )
        .unwrap();
        join(&mut rep, 1);
        rep.prepare(capture(&sim, 1), &[1]).unwrap();
        rep.gather_peer(1).unwrap();
        let entity = rep.actor_entity(ReplicationKey::Enemy(enemy.id)).unwrap();
        let first = rep.scopes_mut(1).unwrap().pending(entity).unwrap().clone();
        rep.scopes_mut(1)
            .unwrap()
            .mark_sent(first.receipt())
            .unwrap();
        rep.scopes_mut(1)
            .unwrap()
            .acknowledge(first.receipt())
            .unwrap();
        for revision in 2..=4 {
            sim.step(DreamInput::default());
            rep.prepare(capture(&sim, revision), &[1]).unwrap();
            rep.gather_peer(1).unwrap();
        }
        let refreshed = rep.scopes_mut(1).unwrap().pending(entity).unwrap();
        assert_eq!(refreshed.payload, first.payload);
        assert!(refreshed.version > first.version);
        assert_eq!(refreshed.end_tick, ServerTick(4));
    }

    #[test]
    fn canonical_public_reuse_matches_uncached_peer_scopes_and_resets_each_capture() {
        let mut sim = party();
        sim.continue_run_for(1);
        sim.continue_run_for(2);
        let mut cached = adapter();
        let mut uncached = adapter();
        uncached.canonical = CanonicalPayloadCache::new(0, 0);
        for rep in [&mut cached, &mut uncached] {
            join(rep, 1);
            join(rep, 2);
        }
        for revision in 1..=3 {
            sim.step_multiplayer(&[]);
            cached.prepare(capture(&sim, revision), &[1, 2]).unwrap();
            uncached.prepare(capture(&sim, revision), &[1, 2]).unwrap();
            assert_eq!(
                cached.canonical_cache_stats(),
                CanonicalCacheStats::default()
            );
            for peer in [1, 2] {
                let eligible = cached.gather_peer(peer).unwrap().clone();
                assert_eq!(&eligible, uncached.gather_peer(peer).unwrap());
                for entry in eligible.entries() {
                    assert_eq!(
                        cached.scopes_mut(peer).unwrap().state(entry.entity()),
                        uncached.scopes_mut(peer).unwrap().state(entry.entity())
                    );
                }
            }
            let stats = cached.canonical_cache_stats();
            assert!(stats.hits > 0 && stats.encodes > 0);
            assert!(stats.retained_bytes <= 2 * 1024 * 1024);
            assert_eq!(uncached.canonical_cache_stats().hits, 0);
            assert!(stats.encodes < uncached.canonical_cache_stats().encodes);
            // Markers reference each recipient's own parent scope and therefore
            // must not share their canonical payload, even when the public root
            // bytes for the cover itself were reused.
            let marker = cached
                .keys
                .iter()
                .find_map(|(&entity, key)| {
                    matches!(key, Key::Actor(ReplicationKey::CoverMarker(_)))
                        .then_some(entity)
                        .filter(|entity| {
                            cached.peers[&1].scopes.state(*entity).is_some()
                                && cached.peers[&2].scopes.state(*entity).is_some()
                        })
                })
                .unwrap();
            let first_marker = cached
                .scopes_mut(1)
                .unwrap()
                .state(marker)
                .unwrap()
                .payload
                .clone();
            assert_ne!(
                first_marker,
                cached.scopes_mut(2).unwrap().state(marker).unwrap().payload
            );
        }
        let actor = cached.actor_entity(ReplicationKey::Hero(2)).unwrap();
        let mut revoked = cached.graph.entity(actor).unwrap().clone();
        revoked.visibility = Visibility::Never;
        cached
            .graph
            .apply(ServerTick(3), Change::Upsert(revoked))
            .unwrap();
        let before = cached.canonical_cache_stats();
        assert!(cached.gather_peer(2).is_err());
        assert!(cached.scopes_mut(2).is_none());
        assert_eq!(
            cached.canonical_cache_stats(),
            before,
            "stale grant must not reach a cache hit"
        );
        assert!(cached.prepare(capture(&sim, 2), &[1, 2]).is_err());
        assert_eq!(
            cached.canonical_cache_stats(),
            CanonicalCacheStats::default()
        );
        assert!(cached.scopes_mut(1).is_none());
    }

    #[test]
    #[ignore = "manual bounded current-host codec measurement; requires DREAMWAKE_CANONICAL_PROBE_OUTPUT"]
    fn canonical_dream_json_probe() {
        use std::{fs::File, io::Write, time::Instant};
        let path =
            std::env::var_os("DREAMWAKE_CANONICAL_PROBE_OUTPUT").expect("explicit output file");
        let mut output = File::create(path).unwrap();
        writeln!(output, "frame,peers,public_requests,uncached_json_ns,cached_json_ns,encodes,hits,retained_bytes,payload_bytes").unwrap();
        let mut sim = DreamSimulation::new(42, false);
        for player in 2..=8 {
            assert!(sim.add_player(player));
        }
        for player in 1..=8 {
            sim.continue_run_for(player);
        }
        let owners: Vec<_> = (1..=8).collect();
        let mut rep = DreamReplication::new(7, 80.0, Limits::default()).unwrap();
        for player in &owners {
            join(&mut rep, *player);
        }
        for revision in 1..=5 {
            sim.step_multiplayer(&[]);
            rep.prepare(capture(&sim, revision), &owners).unwrap();
            let committed = rep.capture.as_ref().unwrap();
            let sources = ApprovedSources::new([]).unwrap();
            let mut work = Vec::new();
            for peer in rep.peers.values() {
                let eligible = peer.eligible.as_ref().unwrap().clone();
                let mut payloads = Vec::new();
                for entry in eligible.entries() {
                    let payload = match rep.keys[&entry.entity()] {
                        Key::Global => ReplicaPayload::Global(committed.global().clone()),
                        Key::Collision => ReplicaPayload::Collision(committed.collision()),
                        Key::Actor(
                            key @ (ReplicationKey::Hero(_)
                            | ReplicationKey::Enemy(_)
                            | ReplicationKey::Cover(_)
                            | ReplicationKey::Platform(_)
                            | ReplicationKey::Damage(_)),
                        ) => ReplicaPayload::Actor(committed.project(key, &sources).unwrap()),
                        _ => continue,
                    };
                    payloads.push((entry.entity(), payload));
                }
                work.push((eligible, payloads));
            }
            let first = &work[0].0;
            let mut cache = CanonicalPayloadCache::new(8192, 2 * 1024 * 1024);
            cache.begin_frame(
                first.frame(),
                first.tick(),
                first.world_revision(),
                first.policy_revision(),
            );
            let reference = || {
                work.iter()
                    .flat_map(|(_, payloads)| {
                        payloads
                            .iter()
                            .map(|(_, payload)| encode_replica(payload).unwrap())
                    })
                    .collect::<Vec<_>>()
            };
            let mut cached = || {
                let mut result = Vec::new();
                for (eligible, payloads) in &work {
                    for (entity, payload) in payloads {
                        result.push(
                            cache
                                .encode(eligible, *entity, || encode_replica(payload))
                                .unwrap(),
                        );
                    }
                }
                result
            };
            // Alternate order to avoid always giving the second path warm caches.
            let (plain, plain_time, shared, shared_time) = if revision % 2 == 1 {
                let start = Instant::now();
                let plain = reference();
                let plain_time = start.elapsed().as_nanos();
                let start = Instant::now();
                let shared = cached();
                let shared_time = start.elapsed().as_nanos();
                (plain, plain_time, shared, shared_time)
            } else {
                let start = Instant::now();
                let shared = cached();
                let shared_time = start.elapsed().as_nanos();
                let start = Instant::now();
                let plain = reference();
                let plain_time = start.elapsed().as_nanos();
                (plain, plain_time, shared, shared_time)
            };
            assert_eq!(plain, shared);
            let stats = cache.stats();
            let bytes: usize = shared.iter().map(Vec::len).sum();
            writeln!(
                output,
                "{revision},8,{},{plain_time},{shared_time},{},{},{},{bytes}",
                shared.len(),
                stats.encodes,
                stats.hits,
                stats.retained_bytes
            )
            .unwrap();
        }
    }

    #[test]
    fn unchanged_actor_delivery_is_dormant_per_peer_and_late_join_still_enters() {
        let mut sim = party();
        assert!(sim.add_player(3));
        let mut rep = adapter();
        join(&mut rep, 1);
        rep.prepare(capture(&sim, 1), &[1, 2]).unwrap();
        rep.gather_peer(1).unwrap();
        let actor = rep.actor_entity(ReplicationKey::Hero(3)).unwrap();
        let scopes = rep.scopes_mut(1).unwrap();
        let receipt = scopes.pending(actor).unwrap().receipt();
        scopes.mark_sent(receipt).unwrap();
        scopes.acknowledge(receipt).unwrap();
        join(&mut rep, 2);
        rep.prepare(capture(&sim, 2), &[1, 2]).unwrap();
        rep.gather_peer(1).unwrap();
        rep.gather_peer(2).unwrap();
        assert_eq!(
            rep.scopes_mut(1).unwrap().phase(actor),
            Some(DeliveryPhase::DormantKnown)
        );
        assert!(rep.scopes_mut(1).unwrap().pending(actor).is_none());
        assert_eq!(
            rep.scopes_mut(2).unwrap().phase(actor),
            Some(DeliveryPhase::Entering)
        );
        assert!(rep.scopes_mut(2).unwrap().pending(actor).is_some());
    }
    #[test]
    fn unchanged_combat_target_refreshes_authority_time_on_bounded_game_cadence() {
        let sim = party();
        let mut rep = adapter();
        join(&mut rep, 1);
        rep.prepare(capture(&sim, 1), &[1, 2]).unwrap();
        rep.gather_peer(1).unwrap();
        let entity = rep.actor_entity(ReplicationKey::Hero(2)).unwrap();
        let first = rep.scopes_mut(1).unwrap().pending(entity).unwrap().clone();
        rep.scopes_mut(1)
            .unwrap()
            .mark_sent(first.receipt())
            .unwrap();
        rep.scopes_mut(1)
            .unwrap()
            .acknowledge(first.receipt())
            .unwrap();
        rep.prepare(capture(&sim, 2), &[1, 2]).unwrap();
        rep.gather_peer(1).unwrap();
        assert!(rep.scopes_mut(1).unwrap().pending(entity).is_none());
        rep.prepare(capture(&sim, 4), &[1, 2]).unwrap();
        rep.gather_peer(1).unwrap();
        let refresh = rep.scopes_mut(1).unwrap().pending(entity).unwrap();
        assert_eq!(refresh.payload, first.payload);
        assert_eq!(refresh.end_tick, ServerTick(4));
        assert!(refresh.version > first.version);
    }
    #[test]
    fn despawn_recreation_changes_generation_and_failed_prepare_blocks_old_bytes() {
        let mut sim = party();
        let mut rep = adapter();
        join(&mut rep, 1);
        rep.prepare(capture(&sim, 1), &[1, 2]).unwrap();
        rep.gather_peer(1).unwrap();
        let old = rep.actor_entity(ReplicationKey::Hero(2)).unwrap();
        assert!(sim.remove_player(2));
        rep.prepare(capture(&sim, 2), &[1]).unwrap();
        rep.gather_peer(1).unwrap();
        assert!(
            rep.scopes_mut(1)
                .unwrap()
                .pending_destroys()
                .any(|destroy| destroy.entity == old)
        );
        assert!(sim.add_player(2));
        rep.prepare(capture(&sim, 3), &[1, 2]).unwrap();
        rep.gather_peer(1).unwrap();
        let new = rep.actor_entity(ReplicationKey::Hero(2)).unwrap();
        assert_eq!(old.index, new.index);
        assert!(new.generation > old.generation);
        assert!(rep.scopes_mut(1).unwrap().pending(new).is_some());
        assert!(rep.prepare(capture(&sim, 2), &[1, 2]).is_err());
        assert!(rep.scopes_mut(1).is_none());
    }
    #[test]
    fn configured_compression_applies_to_shared_canonical_payloads() {
        let sim = party();
        for (compression, marker) in [
            (engine_net::CompressionPolicy::default(), 1),
            (
                engine_net::CompressionPolicy {
                    mode: engine_net::CompressionMode::Off,
                    minimum_bytes: 0,
                },
                0,
            ),
            (
                engine_net::CompressionPolicy {
                    mode: engine_net::CompressionMode::Auto,
                    minimum_bytes: usize::MAX,
                },
                0,
            ),
        ] {
            let mut rep =
                DreamReplication::with_compression(7, 12.0, Limits::default(), compression)
                    .unwrap();
            join(&mut rep, 1);
            join(&mut rep, 2);
            rep.prepare(capture(&sim, 1), &[1, 2]).unwrap();
            rep.gather_peer(1).unwrap();
            rep.gather_peer(2).unwrap();
            let entity = rep.global_entity().unwrap();
            let one = rep
                .scopes_mut(1)
                .unwrap()
                .pending(entity)
                .unwrap()
                .payload
                .clone();
            let two = &rep.scopes_mut(2).unwrap().pending(entity).unwrap().payload;
            assert_eq!(&one, two);
            assert_eq!(one[1], marker);
            let ReplicaPayload::Global(decoded) = decode_replica(&one, None).unwrap() else {
                panic!("global expected");
            };
            assert_eq!(decoded, *capture(&sim, 1).global());
        }
    }

    #[test]
    fn owner_uses_private_checkpoint_without_duplicate_public_hero() {
        let sim = party();
        let mut rep = adapter();
        join(&mut rep, 1);
        join(&mut rep, 2);
        rep.prepare(capture(&sim, 1), &[1, 2]).unwrap();
        let one = rep.actor_entity(ReplicationKey::Hero(1)).unwrap();
        let two = rep.actor_entity(ReplicationKey::Hero(2)).unwrap();
        assert!(rep.gather_peer(1).unwrap().get(one).is_some());
        assert!(rep.gather_peer(2).unwrap().get(two).is_some());
        assert!(rep.peers[&1].sources.contains(&1));
        assert!(rep.peers[&2].sources.contains(&2));
        let owner = rep.owner_entity(1).unwrap();
        assert!(rep.scopes_mut(1).unwrap().pending(owner).is_some());
        assert!(rep.scopes_mut(1).unwrap().pending(one).is_none());
        assert!(rep.scopes_mut(2).unwrap().pending(two).is_none());
        assert!(rep.scopes_mut(1).unwrap().pending(two).is_some());
        assert!(rep.scopes_mut(2).unwrap().pending(one).is_some());
    }

    #[test]
    fn separated_observers_do_not_receive_distant_heroes() {
        let mut sim = party();
        sim.continue_run_for(1);
        sim.continue_run_for(2);
        for _ in 0..180 {
            sim.step_multiplayer(&[
                (
                    1,
                    DreamInput {
                        movement: [-1.0, 0.0],
                        ..Default::default()
                    },
                ),
                (
                    2,
                    DreamInput {
                        movement: [1.0, 0.0],
                        ..Default::default()
                    },
                ),
            ]);
        }
        let cap = capture(&sim, 1);
        let p1 = cap.position(ReplicationKey::Hero(1)).unwrap();
        let p2 = cap.position(ReplicationKey::Hero(2)).unwrap();
        assert!((p1[0] - p2[0]).abs() > 13.0, "{p1:?} {p2:?}");
        let mut rep = adapter();
        join(&mut rep, 1);
        join(&mut rep, 2);
        rep.prepare(cap, &[1, 2]).unwrap();
        let one = rep.actor_entity(ReplicationKey::Hero(1)).unwrap();
        let two = rep.actor_entity(ReplicationKey::Hero(2)).unwrap();
        assert!(rep.gather_peer(1).unwrap().get(two).is_none());
        assert!(rep.gather_peer(2).unwrap().get(one).is_none());
        assert!(rep.scopes_mut(1).unwrap().pending(two).is_none());
    }
}
