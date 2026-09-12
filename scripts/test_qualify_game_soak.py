"""Fake files/processes only. Never launches a game, soak, socket or build."""
import copy
import contextlib
import io
import hashlib
import json
import os
from pathlib import Path
import runpy
import signal
import subprocess
import tempfile
import unittest
from unittest.mock import Mock, patch

MODULE = runpy.run_path(str(Path(__file__).with_name('qualify-game-soak.py')))
Failure = MODULE['Failure']
TraceReader = MODULE['TraceReader']
Limits = MODULE['Limits']
ClientAudit = MODULE['ClientAudit']
ServerAudit = MODULE['ServerAudit']


def row(sequence=1, segment=0, elapsed=1.0, tick=100, scope=1, x=0.0):
    return {'trace_sequence': sequence, 'trace_segment': segment, 'pid': 77, 'owner': 1001,
            'elapsed': elapsed, 'active': True, 'authority_error': None, 'decode_errors': 0,
            'transport_errors': 0, 'stale_epoch_rejections': 0, 'clock_resync_rejections': 0, 'obsolete_rejections': 0, 'decode_failures': 0, 'transport_connection': 1, 'server_instance': 888, 'connection_epoch': 1, 'match_epoch': 1,
            'server_tick': tick, 'estimated_server_tick': tick + 6, 'predicted_tick': tick + 12,
            'gameplay_tick': tick, 'history_bytes': 100, 'scope_bytes': 100, 'staging_bytes': 100,
            'baseline_bytes': 100, 'pending_commands': 2, 'interpolation': {'bytes': 100},
            'delivery': {'maximum_replay_requested': 2, 'replay_cpu_micros': 4}, 'replay_ticks': 2,
            'rtt_ms': 100, 'phase': 'Combat', 'room': 1, 'position': [x, 0], 'action_traces': [],
            'scopes': [{'connection': 1, 'entity': {'index': 1, 'generation': 1}, 'scope': scope}]}


def encoded(value):
    return (json.dumps(value) + '\n').encode()


class TraceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name) / 'left.ndjson'

    def test_partial_append_and_rotation_preserve_one_sequence(self):
        data = encoded(row())
        self.path.write_bytes(data[:30])
        reader = TraceReader(self.path, 2)
        self.assertEqual(reader.poll(), [])
        with self.path.open('ab') as stream: stream.write(data[30:])
        self.assertEqual(reader.poll()[0]['trace_sequence'], 1)
        Path(str(self.path) + '.0001').write_bytes(encoded(row(2, 1, 3, 220)))
        self.assertEqual(reader.poll()[0]['trace_sequence'], 2)
        report = reader.finish()
        self.assertEqual(report['records'], 2)
        self.assertEqual(len(report['files']), 2)
        self.assertEqual(report['bytes'], sum(item['bytes'] for item in report['files']))

    def test_gap_duplicate_wrong_segment_and_nonfinite_fail(self):
        for second in [row(3), row(1), row(2, 1), {**row(2), 'elapsed': float('nan')}]:
            with self.subTest(second=second):
                self.path.write_bytes(encoded(row()) + encoded(second))
                with self.assertRaises(Failure): TraceReader(self.path, 2).poll()

    def test_missing_chunk_and_out_of_budget_chunk_fail(self):
        self.path.write_bytes(encoded(row()))
        future = Path(str(self.path) + '.0002')
        future.write_bytes(encoded(row(2, 2)))
        with self.assertRaises(Failure): TraceReader(self.path, 4).poll()
        with self.assertRaises(Failure): TraceReader(self.path, 2).poll()

    def test_truncation_replacement_and_same_size_rewrite_fail(self):
        self.path.write_bytes(encoded(row()))
        reader = TraceReader(self.path, 2)
        reader.poll()
        self.path.write_bytes(b'')
        with self.assertRaises(Failure): reader.poll()
        self.path.write_bytes(encoded(row()))
        reader = TraceReader(self.path, 2)
        reader.poll()
        replacement = self.path.with_name('replacement')
        replacement.write_bytes(encoded(row()))
        replacement.replace(self.path)
        with self.assertRaises(Failure): reader.poll()
        reader = TraceReader(self.path, 2)
        reader.poll()
        self.path.write_bytes(encoded(row()).replace(b'888', b'889'))
        with self.assertRaises(Failure): reader.finish()

    def test_final_partial_and_closed_segment_mutation_fail(self):
        self.path.write_bytes(encoded(row())[:-1])
        reader = TraceReader(self.path, 2)
        reader.poll()
        with self.assertRaises(Failure): reader.finish()
        self.path.write_bytes(encoded(row()))
        Path(str(self.path) + '.0001').write_bytes(encoded(row(2, 1)))
        reader = TraceReader(self.path, 2)
        reader.poll()
        with self.path.open('ab') as stream: stream.write(b'{}\n')
        with self.assertRaises(Failure): reader.poll()

    def test_final_drain_reads_more_than_one_block_and_audits_cutoff_prefix(self):
        rows = [row(i + 1, elapsed=1 + i * 2, tick=100 + i * 120, x=i) for i in range(4)]
        for value in rows: value['padding'] = 'x' * 700000
        self.path.write_bytes(b''.join(encoded(value) for value in rows))
        reader = TraceReader(self.path, 2)
        cutoff = reader.visible_bytes()
        post = row(5, elapsed=9, tick=580)
        post['transport_errors'] = 1  # intentional post-cutoff shutdown error
        with self.path.open('ab') as stream: stream.write(encoded(post))
        audit = ClientAudit('left', 77, 1001, Limits())
        report = MODULE['finish_trace'](reader, audit, cutoff, 10, 10, 0)
        self.assertEqual(audit.records, 4)
        self.assertEqual(report['records'], 5)
        self.assertEqual(report['post_cutoff_records'], 1)

    def test_unread_bad_row_before_cutoff_is_not_hidden_by_shutdown(self):
        self.path.write_bytes(encoded(row()))
        reader = TraceReader(self.path, 2)
        audit = ClientAudit('left', 77, 1001, Limits())
        for value in reader.poll(): audit.receive(value, 1, 1, 0)
        bad = row(2, elapsed=3, tick=220)
        bad.update(decode_errors=1, decode_failures=1)
        with self.path.open('ab') as stream: stream.write(encoded(bad))
        cutoff = reader.visible_bytes()
        with self.assertRaises(Failure): MODULE['finish_trace'](reader, audit, cutoff, 4, 4, 0)

    def test_total_memory_does_not_retain_completed_records(self):
        self.path.write_bytes(b''.join(encoded(row(i + 1)) for i in range(100)))
        reader = TraceReader(self.path, 1)
        self.assertEqual(len(reader.poll()), 100)
        self.assertEqual(reader.partial, b'')
        self.assertEqual(len(reader.identities), 1)
        self.assertEqual(len(reader.hashers), 1)


class AuditTests(unittest.TestCase):
    def setUp(self):
        self.audit = ClientAudit('left', 77, 1001, Limits())

    def receive(self, value, now=1):
        self.audit.receive(value, now, now, 0)

    def test_combat_progress_and_scope_reentry_without_room_or_match_restart(self):
        self.receive(row())
        absent = row(2, elapsed=3, tick=220, x=2)
        absent['scopes'] = []
        self.receive(absent, 3)
        self.receive(row(3, elapsed=5, tick=340, scope=2, x=4), 5)
        self.audit.check_liveness(6, True)
        self.assertEqual(self.audit.scope_reentries, 1)
        self.assertEqual(self.audit.match_restarts, 0)

    def test_stale_reentry_or_generation_is_rejected(self):
        self.receive(row())
        absent = row(2, elapsed=3, tick=220)
        absent['scopes'] = []
        self.receive(absent, 3)
        with self.assertRaises(Failure): self.receive(row(3, elapsed=5, tick=340), 5)

    def test_new_stream_can_reestablish_scopes(self):
        self.receive(row())
        second = row(2, elapsed=3, tick=220)
        second['connection_epoch'] = 2
        second['scopes'][0]['connection'] = 2
        self.receive(second, 3)
        self.assertEqual(self.audit.stream_changes, 1)

    def test_wrong_owner_pid_epoch_budget_clock_and_executed_replay_fail(self):
        mutations = [('owner', 1002), ('pid', 88), ('history_bytes', 65 * 1024**2),
                     ('decode_errors', 1), ('estimated_server_tick', 1000), ('replay_ticks', 33)]
        for key, value in mutations:
            with self.subTest(key=key):
                bad = row()
                bad[key] = value
                with self.assertRaises(Failure): ClientAudit('left', 77, 1001, Limits()).receive(bad, 1, 1, 0)
        self.receive(row())
        bad = row(2, elapsed=3, tick=99)
        with self.assertRaises(Failure): self.receive(bad, 3)

    def test_stale_epoch_partition_is_visible_but_hard_failures_cannot_hide(self):
        value = row()
        value.update(decode_errors=56, stale_epoch_rejections=56, decode_failures=0)
        self.receive(value)
        self.assertEqual(self.audit.decode_counters['stale_epoch_rejections'], 56)
        bad = row(2, elapsed=3, tick=220)
        bad.update(decode_errors=57, stale_epoch_rejections=56, decode_failures=1)
        with self.assertRaises(Failure): self.receive(bad, 3)
        bad.update(decode_failures=0)
        with self.assertRaises(Failure): ClientAudit('left', 77, 1001, Limits()).receive(bad, 3, 3, 0)

    def test_clock_resync_partition_is_lifetime_visible_and_cannot_mask_hard_errors(self):
        first = row()
        first.update(active=False, decode_errors=3, stale_epoch_rejections=1,
                     clock_resync_rejections=2)
        self.receive(first)
        active = row(2, elapsed=3, tick=220)
        active.update(decode_errors=5, stale_epoch_rejections=1, clock_resync_rejections=4)
        self.receive(active, 3)
        self.assertEqual(self.audit.summary(3)['decode_counters']['clock_resync_rejections'], 4)
        for counters in [dict(decode_errors=6, stale_epoch_rejections=1, clock_resync_rejections=4, decode_failures=1),
                         dict(decode_errors=6, stale_epoch_rejections=1, clock_resync_rejections=4),
                         dict(decode_errors=6, stale_epoch_rejections=3, clock_resync_rejections=3)]:
            with self.subTest(counters=counters):
                audit = ClientAudit('left', 77, 1001, Limits())
                audit.receive(active, 3, 3, 0)
                bad = row(3, elapsed=5, tick=340)
                bad.update(active=False, **counters)
                with self.assertRaises(Failure): audit.receive(bad, 5, 5, 0)

    def test_obsolete_partition_survives_inactive_rows_and_cannot_hide_failures(self):
        first = row()
        first.update(active=False, decode_errors=3, obsolete_rejections=3)
        self.receive(first)
        active = row(2, elapsed=3, tick=220)
        active.update(decode_errors=6, obsolete_rejections=4, stale_epoch_rejections=1,
                      clock_resync_rejections=1)
        self.receive(active, 3)
        self.assertEqual(self.audit.summary(3)['decode_counters']['obsolete_rejections'], 4)
        for counters in [dict(decode_errors=7, obsolete_rejections=4, stale_epoch_rejections=1,
                              clock_resync_rejections=1, decode_failures=1),
                         dict(decode_errors=7, obsolete_rejections=4, stale_epoch_rejections=1,
                              clock_resync_rejections=1),
                         dict(decode_errors=6, obsolete_rejections=3, stale_epoch_rejections=2,
                              clock_resync_rejections=1)]:
            with self.subTest(counters=counters):
                audit = ClientAudit('left', 77, 1001, Limits())
                audit.receive(active, 3, 3, 0)
                bad = row(3, elapsed=5, tick=340)
                bad.update(active=False, decode_issue='Stale', **counters)
                with self.assertRaises(Failure): audit.receive(bad, 5, 5, 0)

    def test_fresh_match_can_reuse_entities_but_old_match_cannot_return(self):
        first = row(scope=7)
        first['scopes'][0]['entity']['generation'] = 9
        self.receive(first)
        fresh = row(2, elapsed=3, tick=220)
        fresh.update(match_epoch=2, gameplay_tick=0)
        self.receive(fresh, 3)
        self.assertEqual(self.audit.match_restarts, 1)
        stale = row(3, elapsed=5, tick=340)
        with self.assertRaises(Failure): self.receive(stale, 5)

    def test_server_startup_reordered_logs_and_recovery_grace(self):
        audit = ServerAudit(Limits())
        template = b'Dreamwake egress peer=%d at_ms=100 control_queue_payload_bytes=10 state_queue_payload_bytes=20 native_udp_ip=Some(stats)'
        audit.bind('left', 1, 77, 1)
        audit.bind('right', 2, 88, 1)
        self.assertFalse(audit.ready(1))
        audit.check_liveness(2)  # bounded initial reporting grace
        audit.receive(template % 1, 2)
        audit.receive(template % 2, 2)
        self.assertTrue(audit.ready(2))
        audit.bind('left', 1, 99, 3)  # stream epoch differs from transport ID
        self.assertTrue(audit.ready(3))
        audit.receive(template % 3, 4)  # replacement log precedes its active trace
        audit.bind('left', 3, 100, 5)
        self.assertTrue(audit.ready(5))
        audit.receive(template % 1, 6)
        self.assertEqual(audit.stale_samples, 1)
        audit.bind('right', 4, 101, 6)
        self.assertFalse(audit.ready(6))
        audit.check_liveness(7)
        with self.assertRaises(Failure): audit.check_liveness(17)

    def test_requested_over_budget_replay_can_be_correctly_rejected(self):
        good = row()
        good['delivery']['maximum_replay_requested'] = 100
        good['replay_ticks'] = 0
        self.receive(good)
        self.assertEqual(self.audit.distributions['replay_ticks'].high, 0)

    def test_sampling_gap_inactive_and_terminal_halt_fail(self):
        self.receive(row())
        with self.assertRaises(Failure): self.receive(row(2, elapsed=20), 20)
        inactive = row(2, elapsed=3)
        inactive['active'] = False
        self.receive(inactive, 3)
        with self.assertRaises(Failure): self.audit.check_liveness(12, True)
        terminal = ClientAudit('left', 77, 1001, Limits())
        for i in range(33):
            value = row(i + 1, elapsed=1 + i * 2, tick=100 + i * 120, x=i)
            value['phase'] = 'Defeat'
            terminal.receive(value, 1 + i * 2, i * 2, 0)
        with self.assertRaises(Failure): terminal.check_liveness(66, True)

    def test_first_inactive_restart_sample_does_not_inherit_combat_phase_age(self):
        # A completed run can restart between trace samples: the next observation
        # is an inactive Intro in a fresh match, with no sampled terminal phase.
        for i in range(41):
            now = 1 + i * 2
            self.receive(row(i + 1, elapsed=now, tick=100 + i * 120, x=i), now)
            self.audit.check_liveness(now, True)
        intro = row(42, elapsed=83, tick=5020)
        intro.update(active=False, phase='Intro', room=0, match_epoch=2,
                     connection_epoch=2, gameplay_tick=0)
        self.receive(intro, 83)
        self.audit.check_liveness(83, True)
        self.assertEqual(self.audit.phase_since, 83)
        self.assertEqual(self.audit.last_active, 81)

        resumed = row(43, elapsed=85, tick=5140, x=1)
        resumed.update(phase='Intro', room=0, match_epoch=2, connection_epoch=2,
                       gameplay_tick=0)
        resumed['scopes'][0]['connection'] = 2
        self.receive(resumed, 85)
        self.audit.check_liveness(85, True)
        self.assertEqual(self.audit.phase_since, 83)
        self.assertEqual(self.audit.match_restarts, 1)
        self.assertEqual(self.audit.stream_changes, 1)

    def test_inactive_phase_changes_do_not_extend_the_inactivity_budget(self):
        self.receive(row())
        for i in range(1, 7):
            now = 1 + i * 2
            inactive = row(i + 1, elapsed=now)
            inactive.update(active=False, phase='Intro', match_epoch=i + 1)
            self.receive(inactive, now)
        with self.assertRaisesRegex(Failure, 'left: inactive'):
            self.audit.check_liveness(13, True)

    def test_inactive_interval_does_not_reset_an_unchanged_terminal_phase(self):
        for i in range(33):
            now = 1 + i * 2
            terminal = row(i + 1, elapsed=now, tick=100 + i * 120, x=i)
            terminal.update(phase='Defeat', active=i != 25)
            self.receive(terminal, now)
            if now <= 61:
                self.audit.check_liveness(now, True)
        self.assertEqual(self.audit.phase_since, 1)
        with self.assertRaisesRegex(Failure, 'autoplay menu/terminal phase halted'):
            self.audit.check_liveness(65, True)

    def test_sampled_action_is_counted_once_and_conflicting_terminal_fails(self):
        value = row()
        action = {'owner': 1001, 'key': {'connection': 1, 'stream': 1, 'command': 1, 'slot': 0},
                  'outcome': {'data': {'status': 'Accepted'}, 'execution_tick': 100}}
        value['action_traces'] = [action]
        self.receive(value)
        value = copy.deepcopy(value)
        value.update(trace_sequence=2, elapsed=3, server_tick=220, gameplay_tick=220, estimated_server_tick=226, predicted_tick=232)
        self.receive(value, 3)
        self.assertEqual(self.audit.accepted_actions, 1)
        value = copy.deepcopy(value)
        value.update(trace_sequence=3, elapsed=5)
        value['action_traces'][0]['outcome']['data']['status'] = 'Rejected'
        with self.assertRaises(Failure): self.receive(value, 5)

    def test_server_real_udp_queue_budget_and_writer_failure(self):
        audit = ServerAudit(Limits())
        template = b'Dreamwake egress peer=%d at_ms=100 control_queue_payload_bytes=10 state_queue_payload_bytes=20 native_udp_ip=Some(stats)'
        audit.receive(template % 1, 1)
        audit.receive(template % 2, 1)
        audit.bind('left', 1, 77, 1)
        audit.bind('right', 2, 88, 1)
        audit.check_liveness(2)
        with self.assertRaises(Failure): audit.check_liveness(12)
        with self.assertRaises(Failure): audit.receive(b'DREAMWAKE_TRACE_ERROR {"kind":"cap"}', 2)
        with self.assertRaises(Failure): audit.receive((template % 1).replace(b'=10 ', b'=999999999 '), 2)
        with self.assertRaises(Failure): audit.receive((template % 1).replace(b'native_udp_ip=Some(stats)', b'native_udp_ip=None'), 2)

    def test_suspension_and_calendar_discontinuity_fail(self):
        guard = MODULE['ClockContinuity'](10, 100, maximum_gap=5, tolerance=1)
        guard.observe(11, 101)
        with self.assertRaises(RuntimeError): guard.observe(20, 110)
        guard = MODULE['ClockContinuity'](10, 100, maximum_gap=5, tolerance=1)
        guard.observe(11, 101.6)
        with self.assertRaises(RuntimeError): guard.observe(12, 103.2)
        guard = MODULE['ClockContinuity'](10, 100, maximum_gap=5, tolerance=1)
        with self.assertRaises(RuntimeError): guard.observe(11, 99)

    def test_rolling_measurements_and_trends_are_bounded(self):
        distribution, trend = MODULE['Distribution'](), MODULE['Trend']()
        for i in range(10000):
            distribution.add(i)
            trend.add(i, i * 2)
        self.assertEqual(len(distribution.values), 256)
        self.assertEqual(distribution.result()['count'], 10000)
        self.assertAlmostEqual(trend.result()['per_hour'], 7200)

    def test_final_cutoff_observes_clocks_after_blocking_io(self):
        for monotonic, reference in [([11, 12], [101, 102]),
                                     ([20], [110]),
                                     ([11, 20], [101, 110]),
                                     ([11, 12], [101, 99]),
                                     ([11, 12], [101, 110])]:
            with self.subTest(monotonic=monotonic, reference=reference):
                guard = MODULE['ClockContinuity'](10, 100, maximum_gap=5, tolerance=1)
                reader = Mock()
                reader.visible_bytes.return_value = 123
                with patch.object(MODULE['time'], 'monotonic', side_effect=monotonic):
                    reference_clock = Mock(side_effect=reference)
                    if reference == [101, 102]:
                        prefixes, cutoff = MODULE['measurement_cutoff']({'left': reader}, guard, reference_clock)
                        self.assertEqual(prefixes, {'left': 123})
                        self.assertEqual(cutoff, 12)
                        self.assertEqual(reference_clock.call_count, 2)
                    else:
                        with self.assertRaises(RuntimeError):
                            MODULE['measurement_cutoff']({'left': reader}, guard, reference_clock)


class PreparationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.workspace = self.root / 'sources/game'
        (self.workspace / 'scripts').mkdir(parents=True)
        (self.workspace / 'Cargo.toml').write_text('[workspace]\n')
        (self.workspace / 'scripts/qualify-soak.py').write_bytes(Path(__file__).with_name('qualify-soak.py').read_bytes())
        files = [{'path': str(path.relative_to(self.root / 'sources')), 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()}
                 for path in self.workspace.rglob('*') if path.is_file()]
        self.manifest = self.root / 'source-manifest.json'
        self.manifest.write_text(json.dumps({'files': files}))
        self.manifest_sha = hashlib.sha256(self.manifest.read_bytes()).hexdigest()
        binaries = self.root / 'binaries'
        binaries.mkdir()
        entries = {}
        for name in ('game_server', 'dreamwake'):
            path = binaries / name
            path.write_bytes(b'FAKE TEST BYTES; NEVER EXECUTE')
            path.chmod(0o500)
            entries[name] = {'sha256': hashlib.sha256(path.read_bytes()).hexdigest(), 'profile': {'opt_level': '3', 'test': False}}
        (self.root / 'native-provenance.json').write_text(json.dumps({
            'source_manifest_sha256': self.manifest_sha, 'artifacts': entries, 'target': 'native',
            'profiles': {name: 'release' for name in entries}}))
        self.args = MODULE['parser']().parse_args(['--source-manifest', str(self.manifest), '--expected-source-sha256', self.manifest_sha,
                                                 '--binary-dir', str(binaries), '--output', str(self.root / 'output'), '--prepare-only'])

    def test_exact_frozen_plan_never_calls_process_or_build(self):
        with patch('subprocess.Popen', side_effect=AssertionError('no processes allowed')):
            identity, unchanged = MODULE['input_provenance'](self.args, self.workspace)
            commands = MODULE['commands'](self.args, identity)
            self.assertTrue(unchanged())
            self.assertEqual(commands['server'][commands['server'].index('--replication-distance') + 1], '80')
            self.assertIn('--autoplay-restart', commands['left'])
            self.assertIn('--network-trace-chunks', commands['right'])
            self.assertEqual(commands['left'][-1], '86580')
            self.assertNotIn('cargo', str(commands))
            self.assertEqual(identity['source_manifest_sha256'], self.manifest_sha)
        (self.workspace / 'Cargo.toml').write_text('changed')
        self.assertFalse(unchanged())

    def test_prepare_only_main_writes_manifest_without_process_or_socket(self):
        main = MODULE['main']
        globals_ = main.__globals__
        original = MODULE['input_provenance']
        argv = ['qualify-game-soak.py', '--source-manifest', str(self.manifest),
                '--expected-source-sha256', self.manifest_sha,
                '--binary-dir', str(self.root / 'binaries'), '--output', str(self.root / 'output'), '--prepare-only']
        with patch.dict(globals_, {'input_provenance': lambda args: original(args, self.workspace)}), \
                patch('sys.argv', argv), patch('subprocess.Popen', side_effect=AssertionError('no process')), \
                patch('socket.socket', side_effect=AssertionError('no socket')), contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(main(), 0)
        result = json.loads((self.root / 'output/report.json').read_text())
        self.assertEqual(result['status'], 'prepared_not_run')
        self.assertFalse(result['real_udp_24h_passed'])
        self.assertEqual(result['identity']['source_manifest_sha256'], self.manifest_sha)
        self.assertEqual(set(result['commands']), {'server', 'left', 'right'})
        self.assertEqual(result['spatial_workload'], {
            'base_replication_distance_metres': 80,
            'separated_waypoints_metres': [[-80, 0], [80, 0]],
            'return_waypoints_metres': [[-1, 2], [1, 2]],
            'waypoint_interval_gameplay_seconds': 12,
            'scope_reentry_gate': 'non-preflight runs require at least one observed scope reentry per client; waypoint intent alone is not evidence'})
        server = result['commands']['server']
        self.assertEqual(int(server[server.index('--replication-distance') + 1]),
                         result['spatial_workload']['base_replication_distance_metres'])

    def test_wrong_manifest_binary_and_provenance_fail(self):
        self.args.expected_source_sha256 = '0' * 64
        with self.assertRaises(Failure): MODULE['input_provenance'](self.args, self.workspace)
        self.args.expected_source_sha256 = self.manifest_sha
        binary = self.root / 'binaries/dreamwake'
        binary.chmod(0o700)
        binary.write_bytes(b'changed')
        with self.assertRaises(Failure): MODULE['input_provenance'](self.args, self.workspace)

    def test_missing_dev_mixed_and_unoptimized_profiles_reject_before_launch(self):
        provenance = self.root / 'native-provenance.json'
        original = json.loads(provenance.read_text())
        mutations = [lambda p: p.pop('profiles'),
                     lambda p: p.update(profiles={'game_server': 'dev', 'dreamwake': 'dev'}),
                     lambda p: p['profiles'].update(dreamwake='dev'),
                     lambda p: p.pop('target'),
                     lambda p: p['artifacts']['dreamwake'].pop('profile'),
                     lambda p: p['artifacts']['dreamwake']['profile'].update(opt_level='0')]
        main = MODULE['main']
        input_provenance = MODULE['input_provenance']
        argv = ['qualify-game-soak.py', '--source-manifest', str(self.manifest),
                '--expected-source-sha256', self.manifest_sha, '--binary-dir', str(self.root / 'binaries'),
                '--output', str(self.root / 'output')]
        for mutation in mutations:
            with self.subTest(mutation=mutation):
                value = copy.deepcopy(original)
                mutation(value)
                provenance.write_text(json.dumps(value))
                with patch.dict(main.__globals__, {'input_provenance': lambda args: input_provenance(args, self.workspace)}), \
                     patch('sys.argv', argv), patch('subprocess.Popen') as process, patch('socket.socket') as socket_, \
                     contextlib.redirect_stderr(io.StringIO()):
                    self.assertEqual(main(), 1)
                self.assertFalse((self.root / 'output').exists())
                process.assert_not_called()
                socket_.assert_not_called()
    def test_short_run_requires_explicit_unqualified_preflight(self):
        self.args.seconds, self.args.warmup_seconds = 60, 10
        with self.assertRaises(Failure): MODULE['validate_args'](self.args)
        self.args.preflight = True
        MODULE['validate_args'](self.args)
        self.args.seconds = 86400
        with self.assertRaises(Failure): MODULE['validate_args'](self.args)

    def test_fake_early_exit_and_owned_cleanup(self):
        process = Mock(pid=123, returncode=1)
        process.poll.return_value = 1
        with patch('subprocess.run', side_effect=AssertionError('no real ps')):
            with self.assertRaises(Failure): MODULE['process_sample'](process)
        process.poll.side_effect = [None, -signal.SIGINT]
        process.wait.return_value = -signal.SIGINT
        with patch('os.killpg') as kill:
            result = MODULE['cleanup']({'left': process})
        kill.assert_called_once_with(123, signal.SIGINT)
        self.assertTrue(result['left']['stopped'])
        self.assertFalse(result['left']['forced_kill'])

    def test_bounded_output_never_silently_discards(self):
        output = MODULE['BoundedFile'](self.root / 'bounded.log', 4)
        try:
            output.write(b'abcd')
            with self.assertRaises(Failure): output.write(b'e')
        finally: output.close()
        self.assertEqual((self.root / 'bounded.log').read_bytes(), b'abcd')


if __name__ == '__main__':
    unittest.main()
