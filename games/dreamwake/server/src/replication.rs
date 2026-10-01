//! Dreamwake's trusted adapter from one committed capture to what each peer
//! may see. The interest graph decides relevance once per publication; this
//! adapter projects the positive public or private value of every relevant
//! entity for each connection, and reports what left its relevance.
use dreamwake_sim::replication::{
    ApprovedSources, CommittedReplication, PublicReplica, Replica, ReplicationKey,
};
use engine_net::{interest::*, types::*};
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
    Owner(u64),
    Actor(ReplicationKey),
}
struct Peer {
    player: u64,
    ownership: OwnershipEpoch,
    epoch: ConnectionEpoch,
    ready: SceneRevision,
    previous: BTreeSet<EntityId>,
    eligible: Option<EligibleSet>,
}
/// One relevant entity for one connection at this publication.
#[derive(Debug, Clone, PartialEq)]
pub struct PeerEntity {
    pub entity: EntityId,
    pub replica: Replica,
    /// Distance to the peer's own hero, when it has one in the world.
    pub distance: Option<f64>,
}
/// What one connection may see now, and what it may no longer see.
#[derive(Debug, Default)]
pub struct PeerGather {
    pub entities: Vec<PeerEntity>,
    pub removed: Vec<EntityId>,
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
    owner_bases: BTreeMap<u64, BTreeSet<EntityId>>,
}
const POLICY: PolicyRevision = PolicyRevision(1);
struct Policy {
    connection: ConnectionId,
    owner: Option<EntityId>,
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
        connection == self.connection && self.owner == Some(source) && self.bases.contains(&target)
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
    pub fn new(epoch: u32, cull_radius: f64, limits: Limits) -> Result<Self, AdapterError> {
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
            owner_bases: BTreeMap::new(),
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
                epoch,
                ready,
                previous: BTreeSet::new(),
                eligible: None,
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
    pub fn reserve_owner(&mut self, owner: u64) -> Result<(EntityId, EntityId), AdapterError> {
        if owner == 0 {
            return Err(error("invalid owner"));
        }
        Ok((
            self.identity(Key::Global)?,
            self.identity(Key::Owner(owner))?,
        ))
    }
    #[cfg(test)]
    pub fn global_entity(&self) -> Option<EntityId> {
        self.ids.get(&Key::Global).copied()
    }
    #[cfg(test)]
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
    #[cfg(test)]
    pub fn actor_entity(&self, key: ReplicationKey) -> Option<EntityId> {
        self.ids.get(&Key::Actor(key)).copied()
    }
    /// Refresh graph once from one committed authority capture, then freeze and
    /// gather every admitted peer. Only sparse candidates are projected per peer.
    pub fn prepare(
        &mut self,
        capture: CommittedReplication,
        active_owners: &[u64],
    ) -> Result<(), AdapterError> {
        for peer in self.peers.values_mut() {
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
                            self.owner_bases
                                .get(&owner)
                                .into_iter()
                                .flatten()
                                .copied()
                                .collect()
                        } else {
                            BTreeSet::new()
                        },
                        state_version: StateVersion(stamp.revision),
                        scene_revision: scene,
                        // The state stream sends only what changed, so relevance
                        // is evaluated for every actor every publication.
                        dormant: false,
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
        }
        self.present = present;
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
                            bases: self.owner_bases.get(&owner).cloned().unwrap_or_default(),
                        },
                    )
                    .map_err(error)?,
            );
        }
        self.capture = Some(capture);
        Ok(())
    }
    /// Project every entity the prepared graph grants this connection. Source
    /// IDs derive solely from independently eligible public heroes and
    /// enemies; dependency closure never reveals a hidden shooter to make an
    /// effect usable. The owner's checkpoint carries `input_continuity` as of
    /// `input_at`, the capture's own tick.
    pub fn gather_peer(
        &mut self,
        connection: u64,
        input_at: ServerTick,
        input_continuity: &engine_net::commands::InputContinuity<dreamwake_sim::DreamInput>,
    ) -> Result<PeerGather, AdapterError> {
        let capture = self.capture.as_ref().ok_or_else(|| error("not prepared"))?;
        if input_at.0 != capture.global().stamp.server_tick {
            return Err(error(
                "owner input continuity differs from committed capture tick",
            ));
        }
        let peer = self
            .peers
            .get_mut(&connection)
            .ok_or_else(|| error("unknown peer"))?;
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
        let source_ids = eligible
            .entries()
            .filter_map(|entry| match self.keys.get(&entry.entity()) {
                Some(Key::Actor(ReplicationKey::Hero(id) | ReplicationKey::Enemy(id))) => Some(*id),
                _ => None,
            })
            .collect::<BTreeSet<u64>>();
        let sources = ApprovedSources::new(source_ids.iter().copied()).map_err(error)?;
        let mut gather = PeerGather::default();
        let mut current = BTreeSet::new();
        for entry in eligible.entries() {
            let id = entry.entity();
            let replica = match self.keys.get(&id).ok_or_else(|| error("missing key"))? {
                Key::Global => Replica::Global(capture.global().clone()),
                Key::Owner(exact_owner) if *exact_owner == peer.player => Replica::Owner(
                    capture
                        .owner_checkpoint(peer.player, peer.ownership.0)
                        .and_then(|checkpoint| {
                            checkpoint.with_input_continuity(input_at, input_continuity.clone())
                        })
                        .map_err(error)?,
                ),
                Key::Owner(_) => return Err(error("private owner grant mismatch")),
                // The private owner checkpoint already carries this hero; keep
                // its grant for source authorization only.
                Key::Actor(ReplicationKey::Hero(hero)) if *hero == peer.player => continue,
                // Markers follow their cover below.
                Key::Actor(ReplicationKey::CoverMarker(_)) => continue,
                Key::Actor(ReplicationKey::Effect(effect)) => {
                    // An opaque lifecycle identity that copies neither the
                    // hidden actor nor its action into a proxy.
                    let proxy = if id.index < (1 << 14) && id.generation < (1 << 18) {
                        (id.generation << 14) | id.index as u32
                    } else {
                        return Err(error("anonymous event identity exhausted"));
                    };
                    match capture.project_event(
                        *effect,
                        &sources,
                        ConnectionId(connection),
                        POLICY,
                        proxy,
                    ) {
                        Some(effect) => Replica::Actor(PublicReplica::Effect(effect)),
                        None => continue,
                    }
                }
                Key::Actor(key) => match capture.project(*key, &sources) {
                    Some(actor) => Replica::Actor(actor),
                    None => continue,
                },
            };
            current.insert(id);
            gather.entities.push(PeerEntity {
                entity: id,
                replica,
                distance: entry.nearest_observer(),
            });
        }
        // Adornments never add a visibility grant to their carrier: both must
        // already be independently eligible.
        for entry in eligible.entries() {
            let id = entry.entity();
            let Some(Key::Actor(ReplicationKey::CoverMarker(marker))) = self.keys.get(&id) else {
                continue;
            };
            let Some(parent) = capture
                .marker_parent(*marker)
                .and_then(|cover| {
                    self.ids
                        .get(&Key::Actor(ReplicationKey::Cover(cover)))
                        .copied()
                })
                .filter(|parent| current.contains(parent))
            else {
                continue;
            };
            let Some(marker) = capture.project_cover_marker(*marker, parent) else {
                continue;
            };
            current.insert(id);
            gather.entities.push(PeerEntity {
                entity: id,
                replica: Replica::Actor(PublicReplica::CoverMarker(marker)),
                distance: entry.nearest_observer(),
            });
        }
        gather.removed = peer.previous.difference(&current).copied().collect();
        peer.previous = current;
        Ok(gather)
    }
}

#[cfg(test)]
#[path = "replication_tests.rs"]
mod tests;
