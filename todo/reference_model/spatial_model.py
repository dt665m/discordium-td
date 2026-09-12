"""Original in-memory contracts for the selected Replication Graph architecture.

Not Unreal source, a production graph, an ECS adapter, a wire codec, or a security
implementation. ConnectionView is already server-authorized input. Floating-point
positions exercise interest queries only, not deterministic physics. Full-scan
comparison belongs in tests, never in ReplicationGraph.gather().
"""
from __future__ import annotations
from dataclasses import dataclass, field
from math import floor, isfinite, dist
from typing import Iterable

Vec3 = tuple[float, float, float]
SPATIAL_ROUTES = frozenset({'static', 'dynamic', 'dormancy'})
ROUTES = SPATIAL_ROUTES | {'none', 'global', 'owner', 'team'}

class SpatialError(ValueError):
    """Invalid input or a deliberately bounded model resource limit."""

@dataclass(frozen=True, order=True)
class EntityKey:
    index: int
    generation: int = 1
    def __post_init__(self):
        if self.index < 0 or self.generation <= 0:
            raise SpatialError('invalid entity identity')

@dataclass(frozen=True)
class Actor:
    key: EntityKey
    position: Vec3 = (0.0, 0.0, 0.0)
    route: str = 'dynamic'
    cull_radius: float = 8.0
    bound_radius: float = 0.0
    owner: int | None = None
    team: int | None = None
    dormant: bool = False
    state_version: int = 1
    dependencies: tuple[EntityKey, ...] = ()

@dataclass(frozen=True)
class ConnectionView:
    connection: int
    observers: tuple[Vec3, ...]
    team: int | None = None
    forbidden: frozenset[EntityKey] = frozenset()
    semantic_grants: frozenset[EntityKey] = frozenset()
    prediction_roots: frozenset[EntityKey] = frozenset()

@dataclass(frozen=True)
class GatherResult:
    eligible: frozenset[EntityKey]
    required: frozenset[EntityKey]
    reasons: dict[EntityKey, frozenset[str]]
    candidate_visits: int
    prediction_admitted: bool
    prediction_error: str | None = None

class ReplicationGraph:
    """Sparse influence cells plus persistent global/owner/team lists.

    The model applies route changes synchronously before prepare. Production must
    use a frozen committed revision and thread-safe lifetime/permission barriers.
    """
    def __init__(self, *, cell_size=32.0, leave_margin=2.0, prefetch=0.0,
                 max_cells_per_actor=256, max_memberships=2_000_000,
                 max_cells=65_536, max_candidates=4096, max_required=128,
                 max_observers=2, max_actors=100_000):
        if not isfinite(cell_size) or cell_size <= 0:
            raise SpatialError('cell size')
        if any(not isfinite(v) or v < 0 for v in (leave_margin, prefetch)):
            raise SpatialError('grid margin')
        if any(v <= 0 for v in (max_cells_per_actor, max_memberships, max_cells,
                                 max_candidates, max_required, max_observers, max_actors)):
            raise SpatialError('positive caps required')
        self.cell_size, self.leave_margin, self.prefetch = cell_size, leave_margin, prefetch
        self.max_cells_per_actor, self.max_memberships = max_cells_per_actor, max_memberships
        self.max_cells, self.max_candidates, self.max_required = max_cells, max_candidates, max_required
        self.max_observers, self.max_actors = max_observers, max_actors
        self.actors: dict[EntityKey, Actor] = {}
        self.cells: dict[tuple[int,int], set[EntityKey]] = {}
        self.footprints: dict[EntityKey, frozenset[tuple[int,int]]] = {}
        self.globals: set[EntityKey] = set()
        self.owners: dict[int, set[EntityKey]] = {}
        self.teams: dict[int, set[EntityKey]] = {}
        self.total_memberships = 0
        self.prepared_frame: int | None = None
        self.prepare_calls = 0
        self.dirty = True

    def cell(self, p: Vec3) -> tuple[int,int]:
        self._position(p)
        return floor(p[0] / self.cell_size), floor(p[1] / self.cell_size)

    @staticmethod
    def _position(p: Vec3):
        if len(p) != 3 or any(not isfinite(v) or abs(v) > 1e9 for v in p):
            raise SpatialError('nonfinite or unsupported world bounds')

    def _coverage(self, actor: Actor) -> frozenset[tuple[int,int]]:
        if actor.route not in SPATIAL_ROUTES:
            return frozenset()
        r = actor.cull_radius + actor.bound_radius + self.leave_margin + self.prefetch
        if not isfinite(r) or r > 1e9:
            raise SpatialError('unsupported radius')
        x, y, _ = actor.position
        x0, x1 = floor((x-r)/self.cell_size), floor((x+r)/self.cell_size)
        y0, y1 = floor((y-r)/self.cell_size), floor((y+r)/self.cell_size)
        count = (x1-x0+1) * (y1-y0+1)
        if count > self.max_cells_per_actor:
            raise SpatialError('actor cell coverage cap')
        return frozenset((cx,cy) for cx in range(x0,x1+1) for cy in range(y0,y1+1))

    def upsert(self, actor: Actor):
        self._position(actor.position)
        if actor.route not in ROUTES or actor.state_version <= 0:
            raise SpatialError('actor route/version')
        if any(not isfinite(v) or v < 0 for v in (actor.cull_radius, actor.bound_radius)):
            raise SpatialError('actor radius')
        if len(actor.dependencies) > self.max_required:
            raise SpatialError('dependency declaration cap')
        if actor.route == 'owner' and actor.owner is None:
            raise SpatialError('owner-only actor missing owner')
        if actor.route == 'team' and actor.team is None:
            raise SpatialError('team-only actor missing team')
        if actor.key not in self.actors and len(self.actors) >= self.max_actors:
            raise SpatialError('actor cap')
        footprint = self._coverage(actor)  # Validate fully before removing old route.
        old = self.footprints.get(actor.key, frozenset())
        memberships = self.total_memberships - len(old) + len(footprint)
        newly_allocated = len(footprint - self.cells.keys())
        freed = sum(1 for c in old-footprint if self.cells[c] == {actor.key})
        if memberships > self.max_memberships or len(self.cells) + newly_allocated - freed > self.max_cells:
            raise SpatialError('graph membership/cell cap')
        self.remove(actor.key)
        self.actors[actor.key] = actor
        self.footprints[actor.key] = footprint
        for c in footprint:
            self.cells.setdefault(c,set()).add(actor.key)
        self.total_memberships += len(footprint)
        if actor.route == 'global': self.globals.add(actor.key)
        if actor.route == 'owner': self.owners.setdefault(actor.owner,set()).add(actor.key)
        if actor.route == 'team': self.teams.setdefault(actor.team,set()).add(actor.key)
        self.dirty = True

    def remove(self, key: EntityKey):
        actor = self.actors.pop(key, None)
        if actor is None: return
        old = self.footprints.pop(key)
        for c in old:
            self.cells[c].remove(key)
            if not self.cells[c]: del self.cells[c]
        self.total_memberships -= len(old)
        self.globals.discard(key)
        for index, route_key in ((self.owners, actor.owner), (self.teams, actor.team)):
            if route_key in index:
                index[route_key].discard(key)
                if not index[route_key]: del index[route_key]
        self.dirty = True

    def prepare(self, frame: int):
        if frame < 0 or (self.prepared_frame is not None and frame <= self.prepared_frame):
            raise SpatialError('prepare requires a new monotonic frame')
        self.prepared_frame = frame
        self.prepare_calls += 1
        self.dirty = False

    @staticmethod
    def permitted(actor: Actor, conn: ConnectionView) -> bool:
        return (actor.route != 'none' and actor.key not in conn.forbidden
                and (actor.route != 'owner' or actor.owner == conn.connection)
                and (actor.route != 'team' or actor.team == conn.team))

    def gather(self, conn: ConnectionView, previous: frozenset[EntityKey] = frozenset()) -> GatherResult:
        if self.prepared_frame is None or self.dirty:
            raise SpatialError('prepare committed changes before gathering')
        if not 1 <= len(conn.observers) <= self.max_observers:
            raise SpatialError('approved observer cap')
        if len(conn.semantic_grants) > self.max_candidates or len(conn.prediction_roots) > self.max_required:
            raise SpatialError('connection declaration cap')
        for p in conn.observers: self._position(p)
        raw: dict[EntityKey,set[str]] = {}
        def offer(keys: Iterable[EntityKey], reason: str):
            for key in keys:
                if key not in raw and len(raw) >= self.max_candidates:
                    raise SpatialError('candidate overload')
                raw.setdefault(key,set()).add(reason)
        offer(self.globals, 'global')
        offer(self.owners.get(conn.connection, ()), 'owner')
        offer(self.teams.get(conn.team, ()), 'team')
        offer(conn.semantic_grants, 'semantic')
        for p in conn.observers: offer(self.cells.get(self.cell(p), ()), 'spatial')
        eligible: dict[EntityKey,set[str]] = {}
        for key, reasons in raw.items():
            actor = self.actors.get(key)
            if actor is None or not self.permitted(actor,conn): continue
            r = actor.cull_radius + actor.bound_radius + self.prefetch
            if key in previous: r += self.leave_margin
            if reasons - {'spatial'} or any(dist(p,actor.position) <= r for p in conn.observers):
                eligible[key] = set(reasons)
        # One admitted prediction closure in this small model, not dynamic groups.
        required: set[EntityKey] = set()
        pending = list(sorted(conn.prediction_roots))
        error = None
        while pending:
            key = pending.pop()
            if key in required: continue
            if len(required) >= self.max_required:
                error = 'dependency_capacity'; break
            actor = self.actors.get(key)
            if actor is None:
                error = 'missing_dependency'; break
            if not self.permitted(actor,conn):
                error = 'denied_dependency'; break
            required.add(key)
            pending.extend(d for d in actor.dependencies if d not in required)
            if len(pending) > self.max_required * self.max_required:
                error = 'dependency_edge_capacity'; break
        if error:
            required.clear()  # No partial prediction admission.
        else:
            for key in required:
                if key not in raw and len(raw) >= self.max_candidates:
                    raise SpatialError('candidate overload in closure')
                raw.setdefault(key,set()).add('required')
                eligible.setdefault(key,set()).add('required')
        return GatherResult(frozenset(eligible), frozenset(required),
                            {k:frozenset(v) for k,v in eligible.items()}, len(raw),
                            error is None, error)

@dataclass(frozen=True)
class ScopeToken:
    connection_epoch: int
    entity: EntityKey
    scope_epoch: int
    representation_revision: int = 1

@dataclass(frozen=True)
class ScopeMessage:
    kind: str
    token: ScopeToken
    state_version: int = 0

@dataclass
class Delivery:
    token: ScopeToken
    desired_version: int
    wants_dormancy: bool
    ready: bool = False
    decoded_version: int = 0
    phase: str = 'entering'
    sent_versions: set[int] = field(default_factory=set)

class ServerScopes:
    """Scope and state-version delivery model for ONE connection.

    No sockets or field/delta encoding. Scope-message identity is tested here;
    exact baseline arithmetic is covered by the existing BaselineStore model.
    Identity counters are bounded until explicit connection reset, not silently
    evicted and reused. Production needs negotiated retirement and repair timers.
    """
    def __init__(self, connection_epoch=1, max_identities=8192, max_inflight_versions=64):
        if connection_epoch <= 0 or max_identities <= 0 or max_inflight_versions <= 0:
            raise SpatialError('scope configuration')
        self.connection_epoch = connection_epoch
        self.max_identities, self.max_inflight_versions = max_identities, max_inflight_versions
        self.counters: dict[EntityKey,int] = {}
        self.current: dict[EntityKey,Delivery] = {}

    def enter(self, key: EntityKey, version=1, representation=1, dormant=False) -> ScopeMessage:
        if version <= 0 or representation <= 0:
            raise SpatialError('scope version')
        if key not in self.counters and len(self.counters) >= self.max_identities:
            raise SpatialError('scope identity cap; reset/admission required')
        epoch = self.counters.get(key,0) + 1
        self.counters[key] = epoch
        d = Delivery(ScopeToken(self.connection_epoch,key,epoch,representation),version,dormant)
        self.current[key] = d
        return self.pending_state(key)

    def needs_state(self, key: EntityKey) -> bool:
        d = self.current[key]
        return d.phase != "leaving" and (not d.ready or d.decoded_version < d.desired_version)

    def pending_state(self, key: EntityKey) -> ScopeMessage:
        d = self.current[key]
        if d.phase == 'leaving': raise SpatialError('scope retiring')
        if d.desired_version not in d.sent_versions and len(d.sent_versions) >= self.max_inflight_versions:
            raise SpatialError('pending version cap')
        d.sent_versions.add(d.desired_version)
        return ScopeMessage('delta' if d.ready else 'enter',d.token,d.desired_version)

    def acknowledge_state(self, msg: ScopeMessage) -> bool:
        d = self.current.get(msg.token.entity)
        if (d is None or d.token != msg.token or d.phase == 'leaving'
            or msg.kind not in ('enter','delta') or msg.state_version not in d.sent_versions):
            return False
        if msg.kind == 'enter': d.ready = True
        d.decoded_version = max(d.decoded_version,msg.state_version)
        d.sent_versions = {v for v in d.sent_versions if v >= d.decoded_version}
        d.phase = ('dormant_known' if d.ready and d.wants_dormancy
                   and d.decoded_version >= d.desired_version else 'active' if d.ready else 'entering')
        return True

    def changed(self, key: EntityKey, version: int, *, dormant=True):
        d = self.current[key]
        if version <= d.desired_version or d.phase == 'leaving':
            raise SpatialError('nonmonotonic version or retiring scope')
        d.desired_version, d.wants_dormancy = version, dormant
        d.phase = 'active' if d.ready else 'entering'

    def exit(self, key: EntityKey) -> ScopeMessage:
        d = self.current[key]
        d.phase = 'leaving'
        return ScopeMessage('exit',d.token)

    def acknowledge_exit(self, msg: ScopeMessage) -> bool:
        d = self.current.get(msg.token.entity)
        if msg.kind != 'exit' or d is None or d.token != msg.token or d.phase != 'leaving':
            return False
        del self.current[msg.token.entity]
        return True

    def reset(self, new_epoch: int):
        if new_epoch <= self.connection_epoch: raise SpatialError('connection epoch must advance')
        self.connection_epoch = new_epoch
        self.counters.clear(); self.current.clear()

class ClientScopes:
    """Model of scope incarnation fences, not a production entity/asset loader."""
    def __init__(self, connection_epoch=1, max_identities=8192):
        self.connection_epoch, self.max_identities = connection_epoch, max_identities
        self.current: dict[EntityKey, tuple[ScopeToken,int]] = {}
        self.closed: dict[EntityKey,int] = {}
        self.destroyed: set[EntityKey] = set()
        self.known: set[EntityKey] = set()

    def apply(self, msg: ScopeMessage) -> bool:
        t, key = msg.token, msg.token.entity
        if t.connection_epoch != self.connection_epoch: return False
        if t.scope_epoch <= 0 or t.representation_revision <= 0: return False
        if msg.kind not in ('enter','delta','exit','destroy'): return False
        if key not in self.known and len(self.known) >= self.max_identities:
            raise SpatialError('client scope identity cap')
        current = self.current.get(key)
        if msg.kind == 'destroy':
            self.known.add(key); self.destroyed.add(key); self.current.pop(key,None)
            return True
        if key in self.destroyed: return False
        if msg.kind == 'exit':
            if current and current[0].scope_epoch == t.scope_epoch and current[0] != t:
                return False
            self.known.add(key)
            self.closed[key] = max(self.closed.get(key,0),t.scope_epoch)
            if current and current[0].scope_epoch <= t.scope_epoch: self.current.pop(key)
            return True
        if t.scope_epoch <= self.closed.get(key,0) or msg.state_version <= 0: return False
        if msg.kind == 'delta':
            if current is None or current[0] != t: return False
        elif current:
            if t.scope_epoch < current[0].scope_epoch: return False
            if t.scope_epoch == current[0].scope_epoch and current[0] != t: return False
        self.known.add(key)
        version = max(current[1],msg.state_version) if current and current[0] == t else msg.state_version
        self.current[key] = (t,version)
        return True

    def reset(self, new_epoch: int):
        if new_epoch <= self.connection_epoch: raise SpatialError('connection epoch must advance')
        self.connection_epoch = new_epoch
        self.current.clear(); self.closed.clear(); self.destroyed.clear(); self.known.clear()
