"""Executable contract model for the engineering handoff.

Original, standard-library-only code. Not production netcode: no sockets, encryption,
3D collision, rendering, adaptive clocks, or performance claims. Integer units are
abstract units per tick. S[k] is the state at the END of tick k.
"""
from __future__ import annotations

from dataclasses import dataclass, fields, replace
from enum import Enum
from typing import Iterable, Mapping


class ProtocolError(ValueError):
    """Invalid or conflicting application message."""


class ResyncRequired(RuntimeError):
    """Safe replay cannot proceed with the available history."""


class MissingBaseline(ResyncRequired):
    pass


@dataclass(frozen=True, order=True)
class ActionKey:
    epoch: int
    stream: int
    command_seq: int
    slot: int = 0


@dataclass(frozen=True)
class Command:
    seq: int
    target: int
    move: int = 0
    fire: bool = False
    epoch: int = 1
    stream: int = 1

    @property
    def key(self) -> ActionKey:
        return ActionKey(self.epoch, self.stream, self.seq)

    def validate(self) -> None:
        # bool is a subclass of int; use exact type checks for hostile payloads.
        for name in ("seq", "target", "move", "epoch", "stream"):
            if type(getattr(self, name)) is not int:
                raise ProtocolError(f"{name} must be an integer")
        if self.seq <= 0 or self.target <= 0 or self.epoch <= 0 or self.stream <= 0:
            raise ProtocolError("nonpositive identity or target")
        if not -1 <= self.move <= 1 or type(self.fire) is not bool:
            raise ProtocolError("invalid input range/type")


@dataclass(frozen=True)
class State:
    tick: int = 0
    x: int = 0
    velocity: int = 0
    ammo: int = 20
    cooldown: int = 0
    rng: int = 7


@dataclass(frozen=True)
class Dependency:
    revision: int = 0
    impulse: int = 0


def step(state: State, command: Command, dependency: Dependency = Dependency()
         ) -> tuple[State, tuple[ActionKey, ...]]:
    """Pure transition S[k-1], U[k] -> S[k]. No external side effects."""
    if command.target != state.tick + 1:
        raise ProtocolError("step must advance exactly one tick")
    velocity = max(-8, min(8, state.velocity + command.move + dependency.impulse))
    cooldown = max(0, state.cooldown - 1)
    ammo, rng = state.ammo, state.rng
    events: tuple[ActionKey, ...] = ()
    if command.fire and ammo > 0 and cooldown == 0:
        ammo -= 1
        cooldown = 3
        rng = (1664525 * rng + 1013904223) & 0xFFFFFFFF
        events = (command.key,)
    return State(command.target, state.x + velocity, velocity, ammo, cooldown, rng), events


class Admission(Enum):
    ACCEPTED = "accepted"
    DUPLICATE = "duplicate"
    LATE = "late"
    FUTURE = "future"


@dataclass(frozen=True)
class Receipt:
    tick: int
    seq: int | None
    status: str


class Server:
    """One-player authoritative command scheduler with bounded future admission.

    Audit history is capped; production uses negotiated history/receipt retention.
    This model retains state history only to make test assertions inspectable.
    """
    def __init__(self, *, epoch: int = 1, future_window: int = 12,
                 held_grace: int = 3, audit_window: int = 256,
                 dependencies: Mapping[int, Dependency] | None = None) -> None:
        self.epoch = epoch
        self.future_window = future_window
        self.held_grace = held_grace
        self.audit_window = audit_window
        self.state = State()
        self.pending: dict[int, Command] = {}
        self.admitted: dict[int, Command] = {}
        self.receipts: dict[int, Receipt] = {}
        self.history = {0: self.state}
        self.dependencies = dict(dependencies or {})
        self.committed_actions: set[ActionKey] = set()
        self.last_move = 0
        self.missing_streak = 0
        self.highest_received_seq = 0

    def submit(self, command: Command) -> Admission:
        command.validate()
        if command.epoch != self.epoch or command.stream != 1:
            raise ProtocolError("wrong connection/stream incarnation")
        previous = self.admitted.get(command.seq)
        if previous is not None:
            if previous != command:
                raise ProtocolError("same sequence, different payload")
            return Admission.DUPLICATE
        if command.target <= self.state.tick:
            return Admission.LATE
        if command.target > self.state.tick + self.future_window:
            return Admission.FUTURE
        occupant = self.pending.get(command.target)
        if occupant is not None:
            raise ProtocolError("cannot replace an admitted target tick")
        self.pending[command.target] = command
        self.admitted[command.seq] = command
        self.highest_received_seq = max(self.highest_received_seq, command.seq)
        return Admission.ACCEPTED

    def advance(self) -> Receipt:
        tick = self.state.tick + 1
        command = self.pending.pop(tick, None)
        if command is None:
            self.missing_streak += 1
            move = self.last_move if self.missing_streak <= self.held_grace else 0
            # A substitute is explicitly not an admitted command or one-shot action.
            command = Command(seq=0, target=tick, move=move, fire=False, epoch=self.epoch)
            receipt = Receipt(tick, None, "substituted")
        else:
            self.missing_streak = 0
            self.last_move = command.move
            receipt = Receipt(tick, command.seq, "executed")
        self.state, events = step(self.state, command, self.dependencies.get(tick, Dependency()))
        for key in events:
            if key in self.committed_actions:
                raise AssertionError("authoritative action executed twice")
            self.committed_actions.add(key)
        self.receipts[tick] = receipt
        self.history[tick] = self.state
        oldest = tick - self.audit_window
        self.admitted = {s: c for s, c in self.admitted.items() if c.target > oldest}
        self.receipts = {t: r for t, r in self.receipts.items() if t > oldest}
        self.history = {t: s for t, s in self.history.items() if t >= oldest}
        return receipt

    @property
    def finalized_through(self) -> int:
        return self.state.tick


class EventJournal:
    """Illustrative idempotent presentation delivery, not an audio/VFX engine."""
    def __init__(self) -> None:
        self.active: set[ActionKey] = set()
        self.ever_started: set[ActionKey] = set()
        self.canceled: set[ActionKey] = set()
        self.confirmed: set[ActionKey] = set()
        self.start_count = 0

    def observe(self, keys: Iterable[ActionKey]) -> None:
        for key in keys:
            if key not in self.ever_started:
                self.start_count += 1
                self.ever_started.add(key)
            self.active.add(key)

    def reconcile(self, old_keys: set[ActionKey], new_keys: set[ActionKey]) -> None:
        for key in old_keys - new_keys - self.confirmed:
            self.active.discard(key)
            self.canceled.add(key)
        self.observe(new_keys)


class Client:
    def __init__(self, *, history_capacity: int = 256, replay_cap: int = 32,
                 dependencies: Mapping[int, Dependency] | None = None) -> None:
        self.state = State()
        self.history: dict[int, State] = {0: self.state}
        self.commands: dict[int, Command] = {}
        self.events: dict[int, tuple[ActionKey, ...]] = {}
        self.dependencies = dict(dependencies or {})
        self.history_capacity = history_capacity
        self.replay_cap = replay_cap
        self.last_authoritative_tick = -1
        self.journal = EventJournal()
        self.last_replayed_ticks: list[int] = []

    def predict(self, command: Command) -> None:
        command.validate()
        if command.target in self.commands:
            raise ProtocolError("prediction input is immutable")
        new_state, events = step(self.state, command, self.dependencies.get(command.target, Dependency()))
        self.commands[command.target] = command
        self.state = new_state
        self.events[command.target] = events
        self.history[command.target] = new_state
        self.journal.observe(events)
        oldest = self.state.tick - self.history_capacity
        self.history = {t: s for t, s in self.history.items() if t >= oldest}
        self.commands = {t: c for t, c in self.commands.items() if t > oldest}
        self.events = {t: e for t, e in self.events.items() if t > oldest}

    def reconcile(self, authoritative: State,
                  dependencies: Mapping[int, Dependency] | None = None) -> bool:
        """Transactional restore and replay. A newer same-tick revision is not modeled."""
        k, p = authoritative.tick, self.state.tick
        if k <= self.last_authoritative_tick:
            return False
        if k > p:
            raise ResyncRequired("checkpoint ahead of prediction")
        if p - k > self.replay_cap:
            raise ResyncRequired("replay CPU budget exceeded")
        replay_ticks = list(range(k + 1, p + 1))
        if any(t not in self.commands for t in replay_ticks):
            raise ResyncRequired("missing replay input")
        deps = dict(self.dependencies)
        if dependencies:
            deps.update(dependencies)
        staging = authoritative
        staged_history = {k: authoritative}
        staged_events: dict[int, tuple[ActionKey, ...]] = {}
        old_keys = {key for t, keys in self.events.items() if t > k for key in keys}
        for tick in replay_ticks:
            staging, events = step(staging, self.commands[tick], deps.get(tick, Dependency()))
            staged_history[tick] = staging
            staged_events[tick] = events
        # No observable mutation before replay has succeeded.
        self.state = staging
        self.dependencies = deps
        self.history.update(staged_history)
        self.events.update(staged_events)
        new_keys = {key for keys in staged_events.values() for key in keys}
        self.journal.reconcile(old_keys, new_keys)
        self.last_authoritative_tick = k
        self.last_replayed_ticks = replay_ticks
        return True


class BaselineStore:
    """Decoded/retained baseline model; refuses implicit eviction.

    Production additionally needs bounded codec parsing and negotiated retirement.
    """
    def __init__(self, capacity: int = 8) -> None:
        self.capacity = capacity
        self.decoded: dict[int, State] = {}
        self.current: State | None = None

    def _publish(self, snapshot_id: int, state: State) -> State:
        if snapshot_id in self.decoded:
            if self.decoded[snapshot_id] != state:
                raise ProtocolError("snapshot ID equivocation")
            return state
        if len(self.decoded) >= self.capacity:
            raise ResyncRequired("retirement/reset required; no implicit baseline eviction")
        self.decoded[snapshot_id] = state
        if self.current is None or state.tick > self.current.tick:
            self.current = state
        return state

    def full(self, snapshot_id: int, state: State) -> State:
        return self._publish(snapshot_id, state)

    def delta(self, snapshot_id: int, baseline_id: int, changes: Mapping[str, int]) -> State:
        base = self.decoded.get(baseline_id)
        if base is None:
            raise MissingBaseline(str(baseline_id))
        allowed = {f.name for f in fields(State)}
        if not set(changes).issubset(allowed) or any(type(v) is not int for v in changes.values()):
            raise ProtocolError("invalid delta fields")
        staged = replace(base, **changes)
        if staged.tick <= base.tick or not 0 <= staged.ammo <= 20 or not 0 <= staged.cooldown <= 3:
            raise ProtocolError("invalid reconstructed state")
        return self._publish(snapshot_id, staged)

    def decoded_ack_ids(self) -> frozenset[int]:
        return frozenset(self.decoded)


@dataclass(frozen=True)
class GroupPart:
    group: int
    revision: int
    tick: int
    members: frozenset[int]
    entity: int
    state: State


class GroupAssembler:
    """Atomic complete-group publication with a finite member cap."""
    def __init__(self, max_members: int = 16) -> None:
        self.max_members = max_members
        self.key: tuple[int, int, int] | None = None
        self.members: frozenset[int] = frozenset()
        self.staging: dict[int, State] = {}
        self.published: dict[int, State] | None = None

    def add(self, part: GroupPart) -> bool:
        if not 0 < len(part.members) <= self.max_members or part.entity not in part.members:
            raise ProtocolError("invalid group manifest")
        if part.state.tick != part.tick:
            raise ProtocolError("mixed-time group member")
        key = (part.group, part.revision, part.tick)
        if self.key is not None and (key != self.key or part.members != self.members):
            raise ProtocolError("explicit discard/reset required for another group version")
        previous = self.staging.get(part.entity)
        if previous is not None and previous != part.state:
            raise ProtocolError("group member equivocation")
        self.key, self.members = key, part.members
        self.staging[part.entity] = part.state
        if set(self.staging) == set(self.members):
            self.published = dict(self.staging)
            return True
        return False


@dataclass(frozen=True)
class HistoricalPose:
    time_ms: int
    x: float
    generation: int = 1
    segment: int = 1
    alive: bool = True


def historical_x(poses: Iterable[HistoricalPose], time_ms: int) -> float:
    ordered = sorted(poses, key=lambda p: p.time_ms)
    for pose in ordered:
        if pose.time_ms == time_ms:
            if not pose.alive:
                raise ResyncRequired("not an eligible living target")
            return pose.x
    for left, right in zip(ordered, ordered[1:]):
        if left.time_ms < time_ms < right.time_ms:
            if (left.generation, left.segment, left.alive) != (right.generation, right.segment, right.alive):
                raise ResyncRequired("cannot interpolate across lifecycle/discontinuity")
            if not left.alive:
                raise ResyncRequired("not an eligible living target")
            alpha = (time_ms - left.time_ms) / (right.time_ms - left.time_ms)
            return left.x + alpha * (right.x - left.x)
    raise ResyncRequired("historical sample unavailable")


def validated_view_time(*, execute_ms: int, requested_view_ms: int,
                        oldest_history_ms: int, max_rewind_ms: int,
                        policy_earliest_ms: int, policy_latest_ms: int) -> int:
    """Clamp an already-defined remote view time; do not subtract RTT again.

    The caller must derive policy bounds from observed timing. This helper does
    not authenticate client timestamps or estimate one-way delay.
    """
    earliest = max(oldest_history_ms, execute_ms - max_rewind_ms, policy_earliest_ms)
    latest = min(execute_ms, policy_latest_ms)
    if earliest > latest:
        raise ResyncRequired("no valid historical query interval")
    return max(earliest, min(latest, requested_view_ms))


def serial_newer_u32(a: int, b: int) -> bool:
    if not 0 <= a <= 0xFFFFFFFF or not 0 <= b <= 0xFFFFFFFF:
        raise ProtocolError("not a u32")
    distance = (a - b) & 0xFFFFFFFF
    if distance == 0x80000000:
        raise ProtocolError("ambiguous half-range distance")
    return 0 < distance < 0x80000000


class SpawnRegistry:
    def __init__(self) -> None:
        self.bindings: dict[ActionKey, tuple[int, int] | None] = {}
        self.predicted_alive: set[ActionKey] = set()
        self.terminal: dict[ActionKey, str] = {}

    def predict(self, key: ActionKey) -> None:
        if key not in self.bindings:
            self.bindings[key] = None
            self.predicted_alive.add(key)

    def local_despawn(self, key: ActionKey) -> None:
        self.predicted_alive.discard(key)

    def accept(self, key: ActionKey, entity: tuple[int, int]) -> None:
        if key not in self.bindings:
            raise ProtocolError("unknown prediction key")
        if self.terminal.get(key) == "rejected":
            raise ProtocolError("terminal outcome conflict")
        old = self.bindings[key]
        if old is not None and old != entity:
            raise ProtocolError("authoritative binding changed")
        self.bindings[key] = entity
        self.terminal[key] = "accepted"
        # Crucially: binding does not resurrect an already locally retired visual.

    def reject(self, key: ActionKey) -> None:
        if key not in self.bindings or self.terminal.get(key) == "accepted":
            raise ProtocolError("unknown key or terminal outcome conflict")
        self.terminal[key] = "rejected"
        self.predicted_alive.discard(key)
