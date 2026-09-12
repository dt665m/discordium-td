#!/usr/bin/env python3
"""Manually run two persistent rendered UDP bots from verified frozen binaries.

No build, source capture, process restart, browser, or generic 50k workload occurs.
Use --prepare-only to validate inputs and write the exact unexecuted run plan.
"""
from __future__ import annotations

import argparse
from collections import OrderedDict, deque
from dataclasses import dataclass, asdict
import hashlib
import json
import math
import os
from pathlib import Path
import re
import runpy
import selectors
import shutil
import signal
import socket
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
MIB = 1024 * 1024
SEGMENT_BYTES = 8 * MIB
LINE_BYTES = MIB
REPORT_BYTES = 64 * MIB
PLAYERS = {'left': 1001, 'right': 1002}
GAMEPLAY_REPLICATION_DISTANCE_METRES = 80
TICK_HZ = 60
CLOCK_HELPERS = runpy.run_path(str(ROOT / 'scripts/qualify-clock.py'))
ClockContinuity = CLOCK_HELPERS['Continuity']
NATIVE_HELPERS = runpy.run_path(str(Path(__file__).with_name('build-native-capture.py')))


class Failure(RuntimeError):
    """An auditable qualification failure, never an invitation to restart."""


def observed_time(continuity, reference_clock) -> float:
    now = time.monotonic()
    continuity.observe(now, reference_clock())
    return now


def measurement_cutoff(readers, continuity, reference_clock):
    observed_time(continuity, reference_clock)
    cutoffs = {name: reader.visible_bytes() for name, reader in readers.items()}
    # The evidence prefix is fixed before the reported cutoff instant. Check
    # again because even filesystem metadata reads can block or span suspend.
    return cutoffs, observed_time(continuity, reference_clock)


def sha(path: Path) -> str:
    value = hashlib.sha256()
    with path.open('rb') as stream:
        for data in iter(lambda: stream.read(MIB), b''):
            value.update(data)
    return value.hexdigest()


def save(path: Path, value: object) -> None:
    temporary = path.with_suffix(path.suffix + '.tmp')
    temporary.write_text(json.dumps(value, indent=2, allow_nan=False) + '\n')
    temporary.replace(path)


def number(value: object, name: str, *, minimum: float | None = None) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value):
        raise Failure(f'{name}: missing or nonfinite number')
    if minimum is not None and value < minimum:
        raise Failure(f'{name}: below {minimum}')
    return value


def integer(value: object, name: str, *, minimum: int = 0) -> int:
    number(value, name, minimum=minimum)
    if not isinstance(value, int):
        raise Failure(f'{name}: expected integer')
    return value


class BoundedFile:
    def __init__(self, path: Path, limit: int):
        self.path, self.limit, self.bytes = path, limit, 0
        self.stream = path.open('xb')

    def write(self, data: bytes) -> None:
        if self.bytes + len(data) > self.limit:
            raise Failure(f'output byte cap exhausted: {self.path.name}')
        self.stream.write(data)
        self.stream.flush()
        self.bytes += len(data)

    def row(self, value: object) -> None:
        self.write((json.dumps(value, allow_nan=False, separators=(',', ':')) + '\n').encode())

    def close(self) -> None:
        self.stream.close()


class Distribution:
    """Exact last-256 rolling quantiles and constant-memory lifetime aggregates."""
    def __init__(self):
        self.values = deque(maxlen=256)
        self.count, self.total = 0, 0.0
        self.low = self.high = None

    def add(self, value: float) -> None:
        number(value, 'measurement')
        self.values.append(value)
        self.count += 1
        self.total += value
        self.low = value if self.low is None else min(self.low, value)
        self.high = value if self.high is None else max(self.high, value)

    def result(self) -> dict:
        ordered = sorted(self.values)
        return {'count': self.count, 'min': self.low, 'max': self.high,
                'mean': self.total / self.count if self.count else None,
                'rolling_samples': len(ordered), 'rolling_window_limit': 256,
                **{f'rolling_p{int(q * 100)}': ordered[max(0, math.ceil(len(ordered) * q) - 1)] if ordered else None
                   for q in (.5, .95, .99)}}


class Trend:
    def __init__(self):
        self.count = 0
        self.mean_x = self.mean_y = self.xx = self.xy = 0.0
        self.first = self.low = self.high = None

    def add(self, seconds: float, value: float) -> None:
        self.count += 1
        dx, dy = seconds - self.mean_x, value - self.mean_y
        self.mean_x += dx / self.count
        self.mean_y += dy / self.count
        self.xx += dx * (seconds - self.mean_x)
        self.xy += dx * (value - self.mean_y)
        if self.first is None:
            self.first = value
        self.low = value if self.low is None else min(self.low, value)
        self.high = value if self.high is None else max(self.high, value)

    def result(self) -> dict:
        return {'samples': self.count, 'first': self.first, 'min': self.low, 'max': self.high,
                'growth_from_first': self.high - self.first if self.count else None,
                'per_hour': self.xy / self.xx * 3600 if self.xx else None}


class TraceReader:
    """Incremental append-only reader; no file or record history grows in memory."""
    def __init__(self, path: Path, chunks: int):
        self.path, self.chunks = path, chunks
        self.segment, self.offset, self.sequence = 0, 0, 0
        self.partial = b''
        self.identities: dict[int, tuple[int, int, int]] = {}
        self.hashers: dict[int, object] = {}
        self.total = 0
        self.last_record_end = 0

    def name(self, segment: int) -> Path:
        return self.path if segment == 0 else Path(f'{self.path}.{segment:04}')

    def _inventory(self) -> None:
        names = list(self.path.parent.glob(self.path.name + '.*'))
        for path in names:
            suffix = path.name[len(self.path.name) + 1:]
            if not re.fullmatch(r'\d{4}', suffix) or not 1 <= int(suffix) < self.chunks:
                raise Failure(f'unexpected trace segment: {path.name}')
            if int(suffix) > self.segment + 1 and not self.name(self.segment + 1).exists():
                raise Failure(f'trace segment gap before {path.name}')
        for index, (device, inode, size) in self.identities.items():
            path = self.name(index)
            if not path.exists() or path.is_symlink():
                raise Failure(f'trace removed or replaced: {path.name}')
            stat = path.stat()
            if (stat.st_dev, stat.st_ino) != (device, inode) or stat.st_size < size:
                raise Failure(f'trace file identity/size changed: {path.name}')
            if index < self.segment and stat.st_size != size:
                raise Failure(f'closed trace segment changed: {path.name}')

    def poll(self) -> list[dict]:
        self._inventory()
        rows, budget = [], 2 * MIB
        while budget:
            path = self.name(self.segment)
            if not path.exists():
                break
            if path.is_symlink() or not path.is_file():
                raise Failure(f'nonordinary trace: {path.name}')
            stat = path.stat()
            if stat.st_size > SEGMENT_BYTES:
                raise Failure(f'trace segment exceeds 8 MiB: {path.name}')
            self.identities[self.segment] = (stat.st_dev, stat.st_ino, stat.st_size)
            self.hashers.setdefault(self.segment, hashlib.sha256())
            with path.open('rb') as stream:
                stream.seek(self.offset)
                data = stream.read(min(budget, stat.st_size - self.offset))
            self.offset += len(data)
            self.total += len(data)
            budget -= len(data)
            self.hashers[self.segment].update(data)
            self.partial += data
            while b'\n' in self.partial:
                line, self.partial = self.partial.split(b'\n', 1)
                if not line or len(line) > LINE_BYTES:
                    raise Failure('empty or oversized trace row')
                try:
                    row = json.loads(line, parse_constant=lambda value: (_ for _ in ()).throw(ValueError(value)))
                except (ValueError, UnicodeError) as error:
                    raise Failure(f'invalid trace JSON: {error}') from error
                if not isinstance(row, dict):
                    raise Failure('trace row must be an object')
                if integer(row.get('trace_sequence'), 'trace_sequence', minimum=1) != self.sequence + 1:
                    raise Failure('missing, duplicate or reordered trace sequence')
                if integer(row.get('trace_segment'), 'trace_segment') != self.segment:
                    raise Failure('trace row has wrong segment identity')
                self.sequence += 1
                row['_trace_start_byte'] = self.last_record_end
                self.last_record_end = self.total - len(self.partial)
                row['_trace_end_byte'] = self.last_record_end
                rows.append(row)
            if len(self.partial) > LINE_BYTES:
                raise Failure('unterminated trace row exceeds byte cap')
            if self.offset != stat.st_size:
                break
            if self.segment + 1 < self.chunks and self.name(self.segment + 1).exists():
                if path.stat().st_size != self.offset:
                    continue
                if self.partial:
                    raise Failure('trace segment ended in a truncated row')
                self.segment += 1
                self.offset = 0
                continue
            break
        return rows

    def visible_bytes(self) -> int:
        self._inventory()
        return sum(self.name(index).stat().st_size for index in range(self.chunks) if self.name(index).exists())

    def finish(self) -> dict:
        self._inventory()
        if self.partial:
            raise Failure('final trace row is truncated')
        if self.sequence == 0:
            raise Failure('trace contains no records')
        files = []
        for segment, (_device, _inode, size) in self.identities.items():
            path = self.name(segment)
            digest = sha(path)
            if digest != self.hashers[segment].hexdigest():
                raise Failure(f'trace bytes changed after reading: {path.name}')
            files.append({'path': path.name, 'bytes': size, 'sha256': digest})
        return {'records': self.sequence, 'bytes': self.total, 'files': files}


def finish_trace(reader: TraceReader, audit: ClientAudit, cutoff_bytes: int, cutoff_time: float,
                 measurement_elapsed: float | None, warmup: float) -> dict:
    """Drain all finite chunks; audit the prefix visible before process shutdown."""
    post_cutoff = 0
    while True:
        before = reader.total
        for row in reader.poll():
            if row['_trace_start_byte'] < cutoff_bytes:
                audit.receive(row, cutoff_time, measurement_elapsed, warmup)
            else:
                post_cutoff += 1
        if reader.total == before:
            break
    return {**reader.finish(), 'post_cutoff_records': post_cutoff}


@dataclass
class Limits:
    inactive_seconds: float = 10.0
    trace_gap_seconds: float = 10.0
    phase_halt_seconds: float = 60.0
    motion_halt_seconds: float = 120.0
    history_bytes: int = 64 * MIB
    scope_bytes: int = 4 * MIB
    staging_bytes: int = 16 * MIB
    baseline_bytes: int = 16 * MIB
    interpolation_bytes: int = 16 * MIB
    pending_commands: int = 256
    known_scopes: int = 8192
    clock_phase_ticks: int = 128
    replay_ticks: int = 32
    server_queue_bytes: int = 16 * MIB


class ClientAudit:
    def __init__(self, name: str, pid: int, owner: int, limits: Limits):
        self.name, self.pid, self.owner, self.limits = name, pid, owner, limits
        self.latest = None
        self.last_received = self.last_active = self.last_progress = self.last_motion = None
        self.last_gameplay_progress = None
        self.previous_gameplay_tick = None
        self.transport_connection = None
        self.decode_counters = {'decode_errors': 0, 'stale_epoch_rejections': 0,
                                'clock_resync_rejections': 0, 'obsolete_rejections': 0, 'decode_failures': 0}
        self.phase_since = 0.0
        self.previous_phase = None
        self.previous_elapsed = None
        self.server_instance = None
        self.connection_epoch = self.match_epoch = 0
        self.previous_tick = None
        self.position = None
        self.scope_history: dict[int, tuple[int, int, bool]] = {}
        self.current_scopes = set()
        self.scoped_entries = self.scoped_exits = self.scope_reentries = 0
        self.stream_changes = self.match_restarts = self.motion_changes = 0
        self.accepted_actions = 0
        self.last_accepted_action = None
        self.action_outcomes = OrderedDict()
        self.active_records = self.records = 0
        self.first_tick = self.first_tick_elapsed = None
        self.distributions: dict[str, Distribution] = {}
        self.clock_trend = Trend()

    def measurement(self, key: str, value: float) -> None:
        self.distributions.setdefault(key, Distribution()).add(value)

    def receive(self, row: dict, now: float, measurement_elapsed: float | None, warmup: float) -> None:
        if integer(row.get('pid'), 'pid', minimum=1) != self.pid:
            raise Failure(f'{self.name}: trace PID changed')
        elapsed = number(row.get('elapsed'), 'elapsed', minimum=0)
        if self.previous_elapsed is not None and not 0 < elapsed - self.previous_elapsed <= self.limits.trace_gap_seconds:
            raise Failure(f'{self.name}: trace elapsed regressed or sampling stopped')
        self.previous_elapsed = elapsed
        self.last_received = now
        self.records += 1
        if not isinstance(row.get('active'), bool):
            raise Failure(f'{self.name}: active flag missing')
        self.latest = row
        counters = {key: integer(row.get(key), key) for key in self.decode_counters}
        if counters['decode_errors'] != (counters['stale_epoch_rejections']
                                         + counters['clock_resync_rejections']
                                         + counters['obsolete_rejections'] + counters['decode_failures']):
            raise Failure(f'{self.name}: decode counter partition is inconsistent')
        if any(counters[key] < self.decode_counters[key] for key in counters):
            raise Failure(f'{self.name}: lifetime decode counters regressed')
        self.decode_counters = counters
        if counters['decode_failures']:
            raise Failure(f'{self.name}: hard decode failure')
        if integer(row.get('transport_errors'), 'transport_errors'):
            raise Failure(f'{self.name}: transport error')
        if row.get('phase') not in ('Intro', 'Combat', 'Reward', 'Rest', 'Transition', 'Victory', 'Defeat'):
            raise Failure(f'{self.name}: unknown gameplay phase')
        # Restart/bootstrap can briefly be inactive. Track the observed phase
        # before returning so its deadline never inherits the prior Combat age.
        # This does not refresh the independent last-active/progress deadlines.
        marker = (row['phase'], row.get('room'), row.get('match_epoch'))
        if marker != self.previous_phase:
            self.previous_phase, self.phase_since = marker, now
        if not row['active']:
            return
        self.active_records += 1
        self.last_active = now
        if integer(row.get('owner'), 'owner', minimum=1) != self.owner:
            raise Failure(f'{self.name}: wrong authoritative owner')
        if row.get('authority_error') is not None:
            raise Failure(f'{self.name}: authoritative scope/checkpoint error')
        instance = integer(row.get('server_instance'), 'server_instance', minimum=1)
        if self.server_instance is not None and instance != self.server_instance:
            raise Failure(f'{self.name}: server instance changed')
        self.server_instance = instance
        self.transport_connection = integer(row.get('transport_connection'), 'transport_connection', minimum=1)
        connection = integer(row.get('connection_epoch'), 'connection_epoch', minimum=1)
        epoch = integer(row.get('match_epoch'), 'match_epoch', minimum=1)
        if connection < self.connection_epoch or epoch < self.match_epoch:
            raise Failure(f'{self.name}: connection/match epoch regressed')
        if connection != self.connection_epoch:
            self.stream_changes += int(self.connection_epoch != 0)
            self.connection_epoch = connection
            self.scope_history.clear()
            self.current_scopes.clear()
        if epoch != self.match_epoch:
            self.match_restarts += int(self.match_epoch != 0)
            self.match_epoch = epoch
            self.scope_history.clear()
            self.current_scopes.clear()
            self.previous_gameplay_tick = None
        tick = integer(row.get('server_tick'), 'server_tick')
        if self.previous_tick is not None and tick < self.previous_tick:
            raise Failure(f'{self.name}: authoritative server tick regressed')
        if tick != self.previous_tick:
            self.last_progress = now
        self.previous_tick = tick
        gameplay_tick = integer(row.get('gameplay_tick'), 'gameplay_tick')
        if self.previous_gameplay_tick is not None and gameplay_tick < self.previous_gameplay_tick:
            raise Failure(f'{self.name}: gameplay tick regressed within match')
        if gameplay_tick != self.previous_gameplay_tick:
            self.last_gameplay_progress = now
        self.previous_gameplay_tick = gameplay_tick
        if measurement_elapsed is not None and self.first_tick is None:
            self.first_tick, self.first_tick_elapsed = tick, measurement_elapsed
        for key in ('history_bytes', 'scope_bytes', 'staging_bytes', 'baseline_bytes', 'pending_commands'):
            value = integer(row.get(key), key)
            if value > getattr(self.limits, key):
                raise Failure(f'{self.name}: {key} budget exceeded')
            self.measurement(key, value)
        interpolation = row.get('interpolation')
        if not isinstance(interpolation, dict):
            raise Failure(f'{self.name}: interpolation counters missing')
        if integer(interpolation.get('bytes'), 'interpolation.bytes') > self.limits.interpolation_bytes:
            raise Failure(f'{self.name}: interpolation budget exceeded')
        self.measurement('interpolation_bytes', interpolation['bytes'])
        delivery = row.get('delivery')
        if not isinstance(delivery, dict):
            raise Failure(f'{self.name}: delivery counters missing')
        replay = integer(row.get('replay_ticks'), 'replay_ticks')
        if replay > self.limits.replay_ticks:
            raise Failure(f'{self.name}: replay tick limit exceeded')
        self.measurement('replay_requested', integer(delivery.get('maximum_replay_requested'), 'maximum_replay_requested'))
        self.measurement('replay_ticks', replay)
        self.measurement('replay_cpu_micros', number(delivery.get('replay_cpu_micros'), 'replay_cpu_micros', minimum=0))
        estimated = integer(row.get('estimated_server_tick'), 'estimated_server_tick')
        predicted = integer(row.get('predicted_tick'), 'predicted_tick')
        phase = estimated - tick
        if abs(phase) > self.limits.clock_phase_ticks or abs(predicted - estimated) > self.limits.clock_phase_ticks:
            raise Failure(f'{self.name}: estimated/checkpoint or predicted/estimated phase exceeded budget')
        self.measurement('estimated_minus_checkpoint_ticks', phase)
        self.measurement('predicted_minus_estimated_ticks', predicted - estimated)
        self.measurement('rtt_ms', number(row.get('rtt_ms'), 'rtt_ms', minimum=0))
        if measurement_elapsed is not None and measurement_elapsed >= warmup:
            self.clock_trend.add(measurement_elapsed, phase)
        actions = row.get('action_traces')
        if not isinstance(actions, list) or len(actions) > 8:
            raise Failure(f'{self.name}: bounded action window missing or exceeded')
        for action in actions:
            if not isinstance(action, dict) or action.get('owner') != self.owner:
                raise Failure(f'{self.name}: wrong sampled action owner')
            outcome = action.get('outcome')
            if outcome is None:
                continue
            key = json.dumps(action.get('key'), sort_keys=True, separators=(',', ':'))
            value = json.dumps(outcome, sort_keys=True, separators=(',', ':'))
            previous = self.action_outcomes.get(key)
            if previous is not None and previous != value:
                raise Failure(f'{self.name}: sampled terminal action outcome changed')
            if previous is None:
                self.action_outcomes[key] = value
                if len(self.action_outcomes) > 1024:
                    self.action_outcomes.popitem(last=False)
                if isinstance(outcome, dict) and outcome.get('data', {}).get('status') == 'Accepted':
                    self.accepted_actions += 1
                    self.last_accepted_action = now
        if self.last_accepted_action is None:
            self.last_accepted_action = now
        position = row.get('position')
        if not isinstance(position, list) or len(position) != 2:
            raise Failure(f'{self.name}: invalid position')
        position = tuple(number(value, 'position') for value in position)
        if self.position is None or math.dist(position, self.position) >= .05:
            self.last_motion = now
            self.motion_changes += int(self.position is not None)
            self.position = position
        scopes = row.get('scopes')
        if not isinstance(scopes, list) or len(scopes) > self.limits.known_scopes:
            raise Failure(f'{self.name}: invalid or excessive scopes')
        current = set()
        for scope in scopes:
            try:
                index = integer(scope['entity']['index'], 'entity index', minimum=1)
                generation = integer(scope['entity']['generation'], 'entity generation', minimum=1)
                incarnation = integer(scope['scope'], 'scope incarnation', minimum=1)
                scope_connection = integer(scope['connection'], 'scope connection', minimum=1)
            except (TypeError, KeyError) as error:
                raise Failure('malformed scope identity') from error
            if scope_connection != connection or index in current:
                raise Failure(f'{self.name}: stale connection or duplicate entity scope')
            old = self.scope_history.get(index)
            if old and (generation < old[0] or (generation == old[0] and (incarnation < old[1] or (old[2] and incarnation <= old[1])))):
                raise Failure(f'{self.name}: stale scope or generation resurrected')
            if index not in self.current_scopes:
                self.scoped_entries += 1
                self.scope_reentries += int(old is not None)
            current.add(index)
            self.scope_history[index] = (generation, incarnation, False)
        for index in self.current_scopes - current:
            old = self.scope_history[index]
            self.scope_history[index] = (old[0], old[1], True)
            self.scoped_exits += 1
        self.current_scopes = current
        if len(self.scope_history) > self.limits.known_scopes:
            raise Failure(f'{self.name}: sampled scope history cap exceeded')

    def check_liveness(self, now: float, measuring: bool) -> None:
        if not measuring:
            return
        for at, limit, label in [(self.last_received, self.limits.trace_gap_seconds, 'trace stopped'),
                                 (self.last_active, self.limits.inactive_seconds, 'inactive'),
                                 (self.last_progress, self.limits.inactive_seconds, 'server tick halted'),
                                 (self.last_motion, self.limits.motion_halt_seconds, 'bot motion halted'),
                                 (self.last_accepted_action, 300, 'no new sampled accepted action within 300 seconds')]:
            if at is None or now - at > limit:
                raise Failure(f'{self.name}: {label}')
        if self.latest and self.latest.get('phase') == 'Combat' and (self.last_gameplay_progress is None or now - self.last_gameplay_progress > self.limits.inactive_seconds):
            raise Failure(f'{self.name}: gameplay tick halted during combat')
        if self.latest and self.latest.get('phase') != 'Combat' and now - self.phase_since > self.limits.phase_halt_seconds:
            raise Failure(f'{self.name}: autoplay menu/terminal phase halted')

    def summary(self, measurement_elapsed: float | None) -> dict:
        duration = measurement_elapsed - self.first_tick_elapsed if self.first_tick_elapsed is not None and measurement_elapsed is not None else 0
        return {'records': self.records, 'active_records': self.active_records,
                'connection_epoch': self.connection_epoch, 'match_epoch': self.match_epoch,
                'transport_connection': self.transport_connection, 'decode_counters': self.decode_counters,
                'stream_changes': self.stream_changes, 'match_restarts': self.match_restarts,
                'motion_changes': self.motion_changes, 'sampled_accepted_actions': self.accepted_actions, 'scope_entries_observed': self.scoped_entries,
                'scope_exits_observed': self.scoped_exits, 'scope_reentries_observed': self.scope_reentries,
                'server_ticks_per_second': (self.previous_tick - self.first_tick) / duration if duration > 0 and self.previous_tick is not None else None,
                'clock_phase_trend': self.clock_trend.result(),
                'measurements': {key: value.result() for key, value in self.distributions.items()}}


class ServerAudit:
    def __init__(self, limits: Limits):
        self.limits = limits
        self.peers: dict[int, dict] = {}
        self.expected: dict[str, tuple[int, int, float]] = {}
        self.retired = deque(maxlen=16)
        self.samples = self.native_samples = self.stale_samples = 0

    def bind(self, name: str, transport: int, epoch: int, now: float) -> None:
        previous = self.expected.get(name)
        if previous and previous[:2] == (transport, epoch):
            return
        self.expected[name] = (transport, epoch, now)
        if previous and previous[0] != transport:
            self.retired.append(previous[0])
            self.peers.pop(previous[0], None)
        if len(self.expected) > 2 or len({value[0] for value in self.expected.values()}) != len(self.expected):
            raise Failure('client transport identities are not two distinct peers')

    def receive(self, line: bytes, now: float) -> None:
        if b'DREAMWAKE_TRACE_ERROR' in line:
            raise Failure('client reported trace writer failure')
        if b'Dreamwake egress peer=' not in line:
            return
        fields = {key.decode(): int(value) for key, value in re.findall(rb'([a-z_]+)=(\d+)', line)}
        for name in ('peer', 'at_ms', 'control_queue_payload_bytes', 'state_queue_payload_bytes'):
            if name not in fields:
                raise Failure('server egress sample missing fields')
        if any(fields[name] > self.limits.server_queue_bytes for name in ('control_queue_payload_bytes', 'state_queue_payload_bytes')):
            raise Failure('server transport queue budget exceeded')
        if b'native_udp_ip=Some(' not in line:
            raise Failure('server sample is not native UDP transport')
        self.native_samples += 1
        self.samples += 1
        current = {value[0] for value in self.expected.values()}
        if fields['peer'] in self.retired and fields['peer'] not in current:
            self.stale_samples += 1
            return
        # Retain at most four latest candidates, covering log-before-trace arrival
        # for two replacement transports; active bindings are never evicted.
        if fields['peer'] not in self.peers and len(self.peers) >= 4:
            candidates = [key for key in self.peers if key not in current]
            if not candidates:
                raise Failure('server peer accounting exceeded bound')
            oldest = min(candidates, key=lambda key: self.peers[key]['observed_monotonic'])
            del self.peers[oldest]
        fields['observed_monotonic'] = now
        self.peers[fields['peer']] = fields

    def ready(self, now: float) -> bool:
        return len(self.expected) == 2 and all(peer in self.peers and now - self.peers[peer]['observed_monotonic'] <= 10
                                                for peer, _epoch, _changed in self.expected.values())

    def check_liveness(self, now: float) -> None:
        if len(self.expected) != 2:
            raise Failure('both active client transport bindings are required')
        for peer, _epoch, changed in self.expected.values():
            fresh = peer in self.peers and now - self.peers[peer]['observed_monotonic'] <= 10
            if not fresh and now - changed > 10:
                raise Failure('current native UDP egress stream missing after reporting grace')


def process_sample(process: subprocess.Popen) -> tuple[int, str]:
    if process.poll() is not None:
        raise Failure(f'owned process exited early: PID {process.pid}, code {process.returncode}')
    result = subprocess.run(['ps', '-o', 'pid=,rss=,lstart=,comm=', '-p', str(process.pid)],
                            capture_output=True, text=True, timeout=3, check=True)
    parts = result.stdout.strip().split(maxsplit=7)
    if len(parts) != 8 or int(parts[0]) != process.pid:
        raise Failure(f'process identity/RSS unavailable for PID {process.pid}')
    return int(parts[1]) * 1024, ' '.join(parts[2:])


def parser() -> argparse.ArgumentParser:
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument('--source-manifest', type=Path, required=True)
    cli.add_argument('--expected-source-sha256', required=True)
    cli.add_argument('--source-root', type=Path)
    cli.add_argument('--binary-dir', type=Path, required=True)
    cli.add_argument('--binary-provenance', type=Path)
    cli.add_argument('--output', type=Path, required=True)
    cli.add_argument('--prepare-only', action='store_true')
    cli.add_argument('--preflight', action='store_true', help='short unqualified operator check, never a 24h pass')
    cli.add_argument('--seconds', type=int, default=86400)
    cli.add_argument('--startup-seconds', type=int, default=120)
    cli.add_argument('--warmup-seconds', type=int, default=300)
    cli.add_argument('--report-seconds', type=int, default=60)
    cli.add_argument('--trace-interval-seconds', type=float, default=2)
    cli.add_argument('--trace-chunks', type=int, default=128)
    cli.add_argument('--max-rss-mib', type=int, default=2048, help='per process resident ceiling')
    cli.add_argument('--max-rss-growth-mib', type=int, default=128)
    cli.add_argument('--max-rss-trend-mib-per-hour', type=float, default=4)
    cli.add_argument('--max-clock-phase-trend-ticks-per-hour', type=float, default=1)
    cli.add_argument('--max-log-mib', type=int, default=256, help='per process stdout/stderr byte ceiling')
    cli.add_argument('--disk-reserve-mib', type=int, default=256)
    cli.add_argument('--http-port', type=int, default=18880)
    cli.add_argument('--udp-port', type=int, default=18881)
    cli.add_argument('--webrtc-port', type=int, default=18882)
    cli.add_argument('--seed', type=int, default=8192, help='game seed; conditioner seed is the compiled default 1')
    return cli


def validate_args(args: argparse.Namespace) -> None:
    if os.name != 'posix':
        raise Failure('process-group supervision requires a POSIX host with ps')
    if not re.fullmatch('[0-9a-f]{64}', args.expected_source_sha256):
        raise Failure('expected source SHA-256 must be 64 lowercase hexadecimal digits')
    if not 30 <= args.seconds <= 604800 or (args.seconds < 86400 and not args.preflight):
        raise Failure('duration must be 30..604800 seconds; shorter than 86400 requires --preflight')
    if args.preflight and args.seconds >= 86400:
        raise Failure('--preflight must be shorter than 86400 seconds')
    if not 30 <= args.startup_seconds <= 600 or not 0 <= args.warmup_seconds < args.seconds:
        raise Failure('invalid startup/warmup duration')
    if not 10 <= args.report_seconds <= 3600 or math.ceil(args.seconds / args.report_seconds) > 10240:
        raise Failure('report interval must be 10..3600 seconds and at most 10240 reports')
    if not math.isfinite(args.trace_interval_seconds) or not .25 <= args.trace_interval_seconds <= 2:
        raise Failure('supervised trace interval must be 0.25..2 seconds')
    if not 1 <= args.trace_chunks <= 128 or not 64 <= args.max_log_mib <= 1024:
        raise Failure('trace chunks or log limit outside bounded range')
    if not 128 <= args.max_rss_mib <= 8192 or not 0 <= args.max_rss_growth_mib <= 8192:
        raise Failure('RSS limits outside bounded range')
    for value in (args.max_rss_trend_mib_per_hour, args.max_clock_phase_trend_ticks_per_hour):
        if not math.isfinite(value) or not 0 <= value <= 1024:
            raise Failure('trend limit outside bounded range')
    if not 64 <= args.disk_reserve_mib <= 8192 or not 0 <= args.seed < 2**64:
        raise Failure('disk reserve or game seed outside bounded range')
    ports = [args.http_port, args.udp_port, args.webrtc_port]
    if len(set(ports)) != 3 or not all(1024 <= value <= 65535 for value in ports):
        raise Failure('three distinct unprivileged ports are required')


def input_provenance(args: argparse.Namespace, workspace: Path = ROOT) -> tuple[dict, callable]:
    manifest = args.source_manifest.resolve()
    source_root = (args.source_root or manifest.parent / 'sources').resolve()
    output = args.output.resolve()
    helpers = runpy.run_path(str(workspace / 'scripts/qualify-soak.py'))
    if sha(manifest) != args.expected_source_sha256:
        raise Failure('source manifest SHA-256 differs from requested identity')
    files, manifest_hash = helpers['frozen_sources'](manifest, source_root, workspace, output)
    binary_dir = args.binary_dir.resolve()
    provenance_path = (args.binary_provenance or binary_dir.parent / 'native-provenance.json').resolve()
    provenance_hash = sha(provenance_path)
    provenance = json.loads(provenance_path.read_text())
    if provenance['source_manifest_sha256'] != manifest_hash:
        raise Failure('binary provenance belongs to a different source manifest')
    try:
        NATIVE_HELPERS['require_optimized_provenance'](provenance)
    except ValueError as error:
        raise Failure(str(error)) from error
    binaries = {}
    for name in ('game_server', 'dreamwake'):
        path = binary_dir / name
        if path.is_symlink() or not path.is_file() or not os.access(path, os.X_OK):
            raise Failure(f'ordinary executable missing: {path}')
        digest = sha(path)
        if digest != provenance['artifacts'][name]['sha256']:
            raise Failure(f'binary hash mismatch: {name}')
        binaries[name] = {'path': str(path), 'sha256': digest, 'bytes': path.stat().st_size,
                          'profile': provenance['artifacts'][name]['profile']}
    identity = {'source_manifest': str(manifest), 'source_manifest_sha256': manifest_hash,
                'source_root': str(source_root), 'source_files': len(files),
                'binary_provenance': str(provenance_path), 'binary_provenance_sha256': provenance_hash,
                'binaries': binaries, 'supervisor_sha256': sha(Path(__file__).resolve())}

    def unchanged() -> bool:
        try:
            return (sha(manifest) == manifest_hash and helpers['unchanged'](source_root, files)
                    and sha(provenance_path) == provenance_hash
                    and all(sha(Path(value['path'])) == value['sha256'] for value in binaries.values()))
        except OSError:
            return False
    return identity, unchanged


def commands(args: argparse.Namespace, identity: dict) -> dict[str, list[str]]:
    output = args.output.resolve()
    base = f'http://127.0.0.1:{args.http_port}'
    binaries = identity['binaries']
    server = [binaries['game_server']['path'], '--identity-store', str(output / 'server.bin'),
              '--seed', str(args.seed), '--replication-distance', str(GAMEPLAY_REPLICATION_DISTANCE_METRES), '--max-clients', '2',
              '--http-bind', f'127.0.0.1:{args.http_port}', '--public-http-base', base,
              '--udp-bind', f'127.0.0.1:{args.udp_port}', '--public-udp-addr', f'127.0.0.1:{args.udp_port}',
              '--webrtc-bind', f'127.0.0.1:{args.webrtc_port}', '--public-webrtc-addr', f'127.0.0.1:{args.webrtc_port}',
              '--net-delay-ms', '50', '--net-jitter-ms', '15', '--net-loss-percent', '3']
    result = {'server': server}
    for name, owner in PLAYERS.items():
        result[name] = [binaries['dreamwake']['path'], '--http-base', base, '--wait-players', '2',
                        '--player-id', str(owner), '--profile-file', str(output / f'{name}-profile.json'),
                        '--spatial-playtest', name, '--autoplay-restart',
                        '--network-trace', str(output / f'{name}.ndjson'),
                        '--network-trace-chunks', str(args.trace_chunks),
                        '--network-trace-interval-seconds', str(args.trace_interval_seconds),
                        '--quit-after', str(args.seconds + args.startup_seconds + 60)]
    return result


def cleanup(processes: dict[str, subprocess.Popen]) -> dict:
    result = {}
    for process in processes.values():
        if process.poll() is None:
            try:
                os.killpg(process.pid, signal.SIGINT)
            except ProcessLookupError:
                pass
    for name, process in processes.items():
        forced = False
        try:
            code = process.wait(timeout=15)
        except subprocess.TimeoutExpired:
            forced = True
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            code = process.wait(timeout=5)
        result[name] = {'pid': process.pid, 'exit_code': code, 'forced_kill': forced,
                        'stopped': process.poll() is not None}
    return result


def supervise(args: argparse.Namespace, plan: dict, unchanged: callable) -> dict:
    output, limits = args.output.resolve(), Limits()
    processes, logs, log_partial, readers, audits = {}, {}, {}, {}, {}
    selector = selectors.DefaultSelector()
    resources = BoundedFile(output / 'resources.jsonl', REPORT_BYTES)
    intervals = BoundedFile(output / 'intervals.jsonl', REPORT_BYTES)
    server_audit = ServerAudit(limits)
    rss_series, rss_trends, process_identities = {}, {}, {}
    report = {**plan, 'status': 'running', 'real_udp_24h_passed': False, 'preflight_passed': False,
              'failures': [], 'cleanup': {}, 'trace_integrity': {}}
    start = time.monotonic()
    reference_clock = (lambda: time.clock_gettime(time.CLOCK_BOOTTIME)) if hasattr(time, 'CLOCK_BOOTTIME') else time.time
    continuity = ClockContinuity(start, reference_clock(), maximum_gap=5, tolerance=1)
    report['independent_clock'] = 'CLOCK_BOOTTIME' if hasattr(time, 'CLOCK_BOOTTIME') else 'time.time fallback; wall-clock jumps invalidate'
    measurement_start = None
    measurement_elapsed = None
    next_poll = next_resource = next_report = 0.0
    reached_deadline = False
    cutoff_monotonic = None
    trace_cutoffs = {}
    raw_log_bytes = 0
    environment = dict(os.environ, RUST_LOG='info', NO_COLOR='1')

    def launch(name: str) -> None:
        logs[name] = BoundedFile(output / f'{name}.log', args.max_log_mib * MIB)
        log_partial[name] = b''
        process = subprocess.Popen(plan['commands'][name], cwd=ROOT, env=environment,
                                   stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                   stderr=subprocess.STDOUT, start_new_session=True)
        processes[name] = process
        os.set_blocking(process.stdout.fileno(), False)
        selector.register(process.stdout, selectors.EVENT_READ, name)
        if name in PLAYERS:
            readers[name] = TraceReader(output / f'{name}.ndjson', args.trace_chunks)
            audits[name] = ClientAudit(name, process.pid, PLAYERS[name], limits)
        rss_series[name], rss_trends[name] = Distribution(), Trend()
        report['pids'] = {key: child.pid for key, child in processes.items()}
        save(output / 'report.json', report)

    def drain_logs(timeout: float = 0) -> None:
        nonlocal raw_log_bytes
        for key, _events in selector.select(timeout):
            name = key.data
            data = os.read(key.fileobj.fileno(), MIB)
            if not data:
                selector.unregister(key.fileobj)
                continue
            logs[name].write(data)
            raw_log_bytes += len(data)
            log_partial[name] += data
            while b'\n' in log_partial[name]:
                line, log_partial[name] = log_partial[name].split(b'\n', 1)
                if len(line) > LINE_BYTES:
                    raise Failure('process log line exceeded bound')
                server_audit.receive(line, time.monotonic())
            if len(log_partial[name]) > LINE_BYTES:
                raise Failure('unterminated process log line exceeded bound')

    try:
        required_free = 2 * args.trace_chunks * SEGMENT_BYTES + 3 * args.max_log_mib * MIB + 2 * REPORT_BYTES + args.disk_reserve_mib * MIB
        if shutil.disk_usage(output).free < required_free:
            raise Failure(f'insufficient disk for finite output budget: need {required_free} free bytes')
        # Fail if selected loopback endpoints are already occupied; do not alter them.
        reservations = []
        try:
            for port, kind in [(args.http_port, socket.SOCK_STREAM), (args.udp_port, socket.SOCK_DGRAM), (args.webrtc_port, socket.SOCK_DGRAM)]:
                sock = socket.socket(socket.AF_INET, kind)
                reservations.append(sock)
                sock.bind(('127.0.0.1', port))
        finally:
            for sock in reservations:
                sock.close()
        launch('server')
        ready = False
        while not ready:
            now = time.monotonic()
            continuity.observe(now, reference_clock())
            if processes['server'].poll() is not None:
                raise Failure('server exited before bootstrap readiness')
            if now - start > 30:
                raise Failure('server bootstrap readiness timed out')
            drain_logs(.1)
            try:
                with socket.create_connection(('127.0.0.1', args.http_port), timeout=.2):
                    ready = True
            except OSError:
                pass
        for name in PLAYERS:
            launch(name)
        while True:
            now = time.monotonic()
            continuity.observe(now, reference_clock())
            drain_logs(.1)
            for name, process in processes.items():
                if process.poll() is not None:
                    raise Failure(f'{name}: process exited before measurement deadline ({process.returncode})')
            if now >= next_poll:
                next_poll = now + 1
                measurement_elapsed = now - measurement_start if measurement_start is not None else None
                for name, reader in readers.items():
                    for row in reader.poll():
                        audits[name].receive(row, now, measurement_elapsed, args.warmup_seconds)
                    audit = audits[name]
                    if audit.latest and audit.latest['active']:
                        server_audit.bind(name, audit.transport_connection, audit.connection_epoch, now)
                if measurement_start is None:
                    if now - start > args.startup_seconds:
                        raise Failure('both bots did not activate before startup deadline')
                    if all(audit.latest and audit.latest['active'] for audit in audits.values()) and server_audit.ready(now):
                        measurement_start = now
                        measurement_elapsed = 0.0
                        report['measurement_started_monotonic'] = now
                        report['measurement_started_unix_seconds'] = time.time()
                        save(output / 'report.json', report)
                for audit in audits.values():
                    audit.check_liveness(now, measurement_start is not None)
                if measurement_start is not None:
                    server_audit.check_liveness(now)
            if now >= next_resource:
                next_resource = now + 5
                if shutil.disk_usage(output).free < args.disk_reserve_mib * MIB:
                    raise Failure('disk reserve exhausted')
                sample = {'supervisor_elapsed_seconds': now - start, 'measurement_elapsed_seconds': measurement_elapsed, 'processes': {}}
                for name, process in processes.items():
                    rss, process_identity = process_sample(process)
                    if name in process_identities and process_identities[name] != process_identity:
                        raise Failure(f'{name}: process executable/start identity changed')
                    process_identities[name] = process_identity
                    if rss > args.max_rss_mib * MIB:
                        raise Failure(f'{name}: RSS ceiling exceeded')
                    rss_series[name].add(rss)
                    if measurement_elapsed is not None and measurement_elapsed >= args.warmup_seconds:
                        rss_trends[name].add(measurement_elapsed, rss)
                        if rss_trends[name].result()['growth_from_first'] > args.max_rss_growth_mib * MIB:
                            raise Failure(f'{name}: post-warmup RSS growth exceeded')
                    sample['processes'][name] = {'pid': process.pid, 'identity': process_identity, 'rss_bytes': rss}
                resources.row(sample)
            if now >= next_report:
                next_report = now + args.report_seconds
                intervals.row({'supervisor_elapsed_seconds': now - start, 'measurement_elapsed_seconds': measurement_elapsed,
                               'clients': {name: audit.summary(measurement_elapsed) for name, audit in audits.items()},
                               'server_egress': {'samples': server_audit.samples, 'latest_peers': server_audit.peers},
                               'rss': {name: {'distribution': rss_series[name].result(), 'after_warmup': rss_trends[name].result()} for name in processes},
                               'trace_bytes': {name: reader.total for name, reader in readers.items()}, 'process_log_bytes': raw_log_bytes})
            now = observed_time(continuity, reference_clock)
            if measurement_start is not None and now - measurement_start >= args.seconds:
                measurement_elapsed = now - measurement_start
                reached_deadline = True
                cutoff_monotonic = now
                report['status'] = 'stopping_at_monotonic_deadline'
                break
    except Exception as error:
        report['failures'].append(str(error))
    except KeyboardInterrupt:
        report['failures'].append('operator interruption; run is incomplete')
    finally:
        try:
            trace_cutoffs, cutoff_monotonic = measurement_cutoff(readers, continuity, reference_clock)
            for name, process in processes.items():
                if process.poll() is not None:
                    raise Failure(f'{name}: process exited before pre-cleanup cutoff ({process.returncode})')
        except Exception as error:
            report['failures'].append(str(error))
            trace_cutoffs = {name: reader.total for name, reader in readers.items()}
            cutoff_monotonic = time.monotonic()
        if measurement_start is not None:
            measurement_elapsed = cutoff_monotonic - measurement_start
        report['measurement_cutoff_monotonic'] = cutoff_monotonic
        report['measurement_trace_cutoff_bytes'] = trace_cutoffs
        report['native_egress_fresh_at_cutoff'] = server_audit.ready(cutoff_monotonic)
        report['cleanup'] = cleanup(processes)
        try:
            # Drain finite remaining pipes and trace writes after owned processes stop.
            drain_deadline = time.monotonic() + 5
            while selector.get_map() and time.monotonic() < drain_deadline:
                drain_logs(.1)
            if selector.get_map():
                raise Failure('owned process log pipes did not close after cleanup')
            for name, reader in readers.items():
                report['trace_integrity'][name] = finish_trace(
                    reader, audits[name], trace_cutoffs[name], cutoff_monotonic,
                    measurement_elapsed, args.warmup_seconds)

        except Exception as error:
            report['failures'].append(str(error))
        for log in logs.values():
            log.close()
        resources.close()
        intervals.close()
        selector.close()
        for process in processes.values():
            if process.stdout:
                process.stdout.close()
    report['clock_continuity'] = continuity.result()
    report['measurement_elapsed_seconds'] = measurement_elapsed
    report['supervisor_elapsed_seconds'] = time.monotonic() - start
    report['clients'] = {name: audit.summary(measurement_elapsed) for name, audit in audits.items()}
    report['rss'] = {name: {'distribution': rss_series[name].result(), 'after_warmup': rss_trends[name].result()} for name in processes}
    report['server_egress_samples'] = server_audit.samples
    report['source_and_binaries_unchanged_after'] = unchanged()
    if not report['source_and_binaries_unchanged_after']:
        report['failures'].append('source, manifest, binary or provenance changed')
    if not reached_deadline:
        report['failures'].append('requested post-activation monotonic duration not completed')
    if reached_deadline:
        report['clients_active_at_cutoff'] = len(audits) == 2 and all(
            audit.latest and audit.latest['active'] and audit.last_received is not None
            and cutoff_monotonic - audit.last_received <= limits.trace_gap_seconds
            for audit in audits.values())
        if not report['clients_active_at_cutoff'] or not report['native_egress_fresh_at_cutoff']:
            report['failures'].append('measurement ended without both active clients and fresh current native UDP egress')
    if any(value['forced_kill'] or not value['stopped'] or value['exit_code'] not in (0, -signal.SIGINT) for value in report['cleanup'].values()):
        report['failures'].append('owned process cleanup was not a clean planned interrupt')
    if not args.preflight and reached_deadline:
        for name, audit in audits.items():
            summary = report['clients'][name]
            if not summary['server_ticks_per_second'] or summary['server_ticks_per_second'] < TICK_HZ * .9:
                report['failures'].append(f'{name}: server tick progress below 90% of 60 Hz')
            if audit.scope_reentries < 1 or audit.motion_changes < 2 or audit.accepted_actions < 1:
                report['failures'].append(f'{name}: persistent scope-reentry/motion/accepted-action workload not observed')
            trend = audit.clock_trend.result()
            if trend['samples'] < 2 or trend['per_hour'] is None or abs(trend['per_hour']) > args.max_clock_phase_trend_ticks_per_hour:
                report['failures'].append(f'{name}: estimated/checkpoint clock-phase trend exceeded or unavailable')
        for name, trend in rss_trends.items():
            result = trend.result()
            if result['samples'] < 2 or result['per_hour'] is None or result['per_hour'] > args.max_rss_trend_mib_per_hour * MIB:
                report['failures'].append(f'{name}: RSS trend exceeded or unavailable')
    passed = reached_deadline and not report['failures']
    report['preflight_passed'] = bool(passed and args.preflight)
    report['real_udp_24h_passed'] = bool(passed and not args.preflight and measurement_elapsed >= 86400)
    report['status'] = 'preflight_passed_unqualified' if report['preflight_passed'] else 'real_udp_24h_scoped_pass' if report['real_udp_24h_passed'] else 'failed_or_incomplete'
    save(output / 'report.json', report)
    return report


def main() -> int:
    cli = parser()
    args = cli.parse_args()
    try:
        validate_args(args)
        identity, unchanged = input_provenance(args)
        output = args.output.resolve()
        if output.exists() and any(output.iterdir()):
            raise Failure('output directory must be new or empty')
        os.umask(0o077)
        output.mkdir(parents=True, exist_ok=True)
        plan = {'format': 1, 'status': 'prepared_not_run', 'identity': identity,
                'host': {'os': os.uname().sysname, 'release': os.uname().release,
                         'machine': os.uname().machine, 'python': sys.version},
                'configuration': {name: str(value) if isinstance(value, Path) else value for name, value in vars(args).items()},
                'limits': asdict(Limits()), 'commands': commands(args, identity),
                'conditioner': {'one_way_delay_ms': 50, 'one_way_jitter_ms': 15, 'independent_loss_percent': 3,
                                'compiled_seed': 1, 'seed_scope': 'peer seeds derive from transport identity; packet ordering and game seed do not imply byte-identical runs'},
                'spatial_workload': {'base_replication_distance_metres': GAMEPLAY_REPLICATION_DISTANCE_METRES,
                                     'separated_waypoints_metres': [[-80, 0], [80, 0]],
                                     'return_waypoints_metres': [[-1, 2], [1, 2]],
                                     'waypoint_interval_gameplay_seconds': 12,
                                     'scope_reentry_gate': 'non-preflight runs require at least one observed scope reentry per client; waypoint intent alone is not evidence'},
                'action_measurement': 'sampled last-eight action windows with a bounded 1024-key terminal consistency check; not exhaustive duplicate-commit proof',
                'clock_measurement': 'estimated_server_tick minus received checkpoint server_tick; includes checkpoint delivery age, not a one-way latency estimate or injected-drift test',
                'quantiles': 'exact last-256-sample rolling window; lifetime min/max/mean; raw client chunks and resource intervals retained',
                'scope': 'two persistent native rendered bots, actual loopback UDP, moderate impairment, sampled lifecycle/clock/resource evidence',
                'not_qualified': ['WebRTC/browser or human profile matrix', 'generic 100-connection/50000-actor N073 graph soak', 'injected positive/negative clock drift', 'assigned reference-hardware or input-to-display quality'],
                'real_udp_24h_passed': False, 'preflight_passed': False}
        save(output / 'run-plan.json', plan)
        if args.prepare_only:
            if not unchanged():
                raise Failure('input drift while preparing run plan')
            save(output / 'report.json', plan)
            print(f'Prepared only; no process launched: {output / "run-plan.json"}')
            return 0
        def interrupted(_signum, _frame):
            raise KeyboardInterrupt
        signal.signal(signal.SIGTERM, interrupted)
        report = supervise(args, plan, unchanged)
        print(f'{report["status"]}: {output / "report.json"}')
        return 0 if report['preflight_passed'] or report['real_udp_24h_passed'] else 1
    except (Failure, OSError, ValueError, KeyError, TypeError) as error:
        print(f'Game soak preparation failed: {error}', file=sys.stderr)
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
