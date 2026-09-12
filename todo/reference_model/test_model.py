"""Run with: python -m unittest discover -s reference_model -v"""
import random
import unittest
from dataclasses import replace

from model import (
    ActionKey, Admission, BaselineStore, Client, Command, Dependency,
    GroupAssembler, GroupPart, HistoricalPose, MissingBaseline,
    ProtocolError, ResyncRequired, Server, SpawnRegistry, State,
    historical_x, serial_newer_u32, step, validated_view_time,
)


class SimulationTests(unittest.TestCase):
    def test_end_of_tick_requires_next_command(self):
        with self.assertRaises(ProtocolError):
            step(State(tick=5), Command(5, 5))

    def test_repeatability(self):
        def run():
            state = State()
            for k in range(1, 101):
                state, _ = step(state, Command(k, k, (k % 3) - 1, k % 4 == 0))
            return state
        self.assertEqual(run(), run())

    def test_restore_every_tick_matches_full_execution(self):
        commands = [Command(k, k, (k % 3) - 1, k % 4 == 0) for k in range(1, 40)]
        states = [State()]
        for command in commands:
            states.append(step(states[-1], command)[0])
        for k, checkpoint in enumerate(states):
            state = checkpoint
            for command in commands[k:]:
                state, _ = step(state, command)
            self.assertEqual(state, states[-1])

    def test_replay_starts_after_checkpoint(self):
        client = Client()
        for k in range(1, 7):
            client.predict(Command(k, k, 1))
        client.reconcile(replace(client.history[3], x=100))
        self.assertEqual(client.last_replayed_ticks, [4, 5, 6])
        self.assertEqual(client.state.x, 100 + 4 + 5 + 6)

    def test_equal_position_different_velocity_is_corrected(self):
        client = Client()
        for k in range(1, 4):
            client.predict(Command(k, k, 0))
        client.reconcile(State(tick=1, x=0, velocity=2))
        self.assertEqual(client.state.x, 4)

    def test_full_state_rng_ammo_and_cooldown_replayed(self):
        client = Client()
        for k in range(1, 8):
            client.predict(Command(k, k, 0, True))
        auth = State(tick=3, ammo=2, cooldown=1, rng=111)
        expected = auth
        for k in range(4, 8):
            expected, _ = step(expected, client.commands[k])
        client.reconcile(auth)
        self.assertEqual(client.state, expected)

    def test_dependency_revision_changes_replay(self):
        client = Client()
        for k in range(1, 4):
            client.predict(Command(k, k))
        client.reconcile(State(tick=1), {2: Dependency(2, 4)})
        self.assertEqual(client.state.x, 8)

    def test_missing_command_requires_resync_transactionally(self):
        client = Client()
        for k in range(1, 4):
            client.predict(Command(k, k, 1))
        del client.commands[2]
        before = client.state
        with self.assertRaises(ResyncRequired):
            client.reconcile(State(tick=1))
        self.assertEqual(client.state, before)

    def test_replay_budget_is_not_history_capacity(self):
        client = Client(replay_cap=2)
        for k in range(1, 5):
            client.predict(Command(k, k))
        with self.assertRaises(ResyncRequired):
            client.reconcile(State(tick=1))

    def test_stale_checkpoint_does_not_rewind_client(self):
        client = Client()
        for k in range(1, 4):
            client.predict(Command(k, k, 1))
        client.reconcile(State(tick=2, x=10))
        before = client.state
        self.assertFalse(client.reconcile(State(tick=1, x=-99)))
        self.assertEqual(client.state, before)

    def test_checkpoint_ahead_requires_resync(self):
        with self.assertRaises(ResyncRequired):
            Client().reconcile(State(tick=100))

    def test_replayed_effect_is_not_played_twice(self):
        client = Client()
        for k in range(1, 5):
            client.predict(Command(k, k, 0, k == 2))
        self.assertEqual(client.journal.start_count, 1)
        client.reconcile(State(tick=1))
        self.assertEqual(client.journal.start_count, 1)

    def test_rejected_prediction_cancels_provisional_effect(self):
        client = Client()
        for k in range(1, 4):
            client.predict(Command(k, k, 0, k == 2))
        client.reconcile(State(tick=1, ammo=0))
        self.assertIn(ActionKey(1, 1, 2), client.journal.canceled)


class AdmissionTests(unittest.TestCase):
    def test_exact_duplicate_no_double_damage(self):
        server = Server()
        command = Command(1, 1, 0, True)
        self.assertEqual(server.submit(command), Admission.ACCEPTED)
        self.assertEqual(server.submit(command), Admission.DUPLICATE)
        server.advance()
        self.assertEqual(server.state.ammo, 19)
        self.assertEqual(len(server.committed_actions), 1)

    def test_same_sequence_changed_payload_rejected(self):
        server = Server()
        server.submit(Command(1, 1, 0))
        with self.assertRaises(ProtocolError):
            server.submit(Command(1, 1, 1))

    def test_target_tick_cannot_be_replaced(self):
        server = Server()
        server.submit(Command(1, 1))
        with self.assertRaises(ProtocolError):
            server.submit(Command(2, 1))

    def test_received_is_not_finalized(self):
        server = Server()
        server.submit(Command(8, 8))
        self.assertEqual(server.highest_received_seq, 8)
        self.assertEqual(server.finalized_through, 0)
        self.assertEqual(server.advance().status, "substituted")
        self.assertEqual(server.finalized_through, 1)

    def test_late_input_cannot_replace_substitute(self):
        server = Server()
        server.advance()
        before = server.state
        self.assertEqual(server.submit(Command(1, 1, 1, True)), Admission.LATE)
        self.assertEqual(server.state, before)

    def test_missing_input_never_repeats_fire_edge(self):
        server = Server()
        server.submit(Command(1, 1, 1, True))
        server.advance()
        for _ in range(12):
            server.advance()
        self.assertEqual(len(server.committed_actions), 1)
        self.assertEqual(server.state.ammo, 19)

    def test_held_movement_grace_is_bounded(self):
        server = Server(held_grace=2)
        server.submit(Command(1, 1, 1))
        for _ in range(5):
            server.advance()
        self.assertEqual(server.history[3].velocity, 3)
        self.assertEqual(server.history[5].velocity, 3)  # no further acceleration

    def test_future_input_queue_is_bounded(self):
        server = Server(future_window=4)
        for k in range(1, 100):
            server.submit(Command(k, k))
        self.assertEqual(len(server.pending), 4)

    def test_old_epoch_cannot_control_new_connection(self):
        with self.assertRaises(ProtocolError):
            Server(epoch=2).submit(Command(1, 1, epoch=1))

    def test_nonfinite_and_wrong_types_rejected(self):
        for move in (float("nan"), float("inf"), True, 100):
            with self.assertRaises(ProtocolError):
                Server().submit(Command(1, 1, move=move))

    def test_input_arrival_does_not_advance_world(self):
        server = Server()
        for k in range(1, 13):
            server.submit(Command(k, k, 1))
        self.assertEqual(server.state.tick, 0)
        server.advance()
        self.assertEqual(server.state.tick, 1)


class SnapshotTests(unittest.TestCase):
    def test_unknown_baseline_never_acked_or_published(self):
        store = BaselineStore()
        store.full(1, State(tick=1))
        before = store.current
        with self.assertRaises(MissingBaseline):
            store.delta(3, 2, {"tick": 3, "x": 100})
        self.assertEqual(store.current, before)
        self.assertEqual(store.decoded_ack_ids(), frozenset({1}))

    def test_valid_delta_uses_exact_baseline(self):
        store = BaselineStore()
        store.full(1, State(tick=1, x=10, velocity=2))
        state = store.delta(2, 1, {"tick": 2, "x": 12})
        self.assertEqual(state, State(tick=2, x=12, velocity=2))

    def test_invalid_delta_is_transactional(self):
        store = BaselineStore()
        store.full(1, State(tick=1))
        with self.assertRaises(ProtocolError):
            store.delta(2, 1, {"tick": 2, "ammo": -100})
        self.assertEqual(store.decoded_ack_ids(), frozenset({1}))

    def test_no_unnegotiated_baseline_eviction(self):
        store = BaselineStore(capacity=1)
        store.full(1, State(tick=1))
        with self.assertRaises(ResyncRequired):
            store.full(2, State(tick=2))
        self.assertEqual(store.decoded_ack_ids(), frozenset({1}))

    def test_snapshot_id_equivocation_rejected(self):
        store = BaselineStore()
        store.full(1, State(tick=1))
        with self.assertRaises(ProtocolError):
            store.full(1, State(tick=1, x=10))

    def test_group_is_not_published_partially(self):
        group = GroupAssembler()
        members = frozenset({1, 2})
        self.assertFalse(group.add(GroupPart(1, 1, 5, members, 1, State(tick=5))))
        self.assertIsNone(group.published)
        self.assertTrue(group.add(GroupPart(1, 1, 5, members, 2, State(tick=5))))
        self.assertEqual(set(group.published), {1, 2})

    def test_group_member_at_different_tick_is_rejected(self):
        with self.assertRaises(ProtocolError):
            GroupAssembler().add(GroupPart(1, 1, 5, frozenset({1}), 1, State(tick=4)))

    def test_group_revision_mismatch_is_rejected(self):
        group = GroupAssembler()
        members = frozenset({1, 2})
        group.add(GroupPart(1, 1, 5, members, 1, State(tick=5)))
        with self.assertRaises(ProtocolError):
            group.add(GroupPart(1, 2, 5, members, 2, State(tick=5)))


class CombatAndLifecycleTests(unittest.TestCase):
    def test_view_time_does_not_subtract_rtt_twice(self):
        result = validated_view_time(execute_ms=1050, requested_view_ms=925,
                                    oldest_history_ms=600, max_rewind_ms=150,
                                    policy_earliest_ms=900, policy_latest_ms=1000)
        self.assertEqual(result, 925)

    def test_rewind_cap_enforced(self):
        result = validated_view_time(execute_ms=1050, requested_view_ms=200,
                                    oldest_history_ms=0, max_rewind_ms=150,
                                    policy_earliest_ms=0, policy_latest_ms=1050)
        self.assertEqual(result, 900)

    def test_history_interpolates_compatible_samples(self):
        self.assertEqual(historical_x([HistoricalPose(0, 0), HistoricalPose(100, 10)], 50), 5)

    def test_history_never_blends_teleport(self):
        with self.assertRaises(ResyncRequired):
            historical_x([HistoricalPose(0, 0), HistoricalPose(100, 10, segment=2)], 50)

    def test_history_never_blends_generations(self):
        with self.assertRaises(ResyncRequired):
            historical_x([HistoricalPose(0, 0), HistoricalPose(100, 10, generation=2)], 50)

    def test_missing_history_has_explicit_failure(self):
        with self.assertRaises(ResyncRequired):
            historical_x([HistoricalPose(100, 0)], 50)

    def test_accept_after_local_despawn_does_not_resurrect(self):
        registry = SpawnRegistry()
        key = ActionKey(1, 1, 3)
        registry.predict(key)
        registry.local_despawn(key)
        registry.accept(key, (99, 2))
        registry.accept(key, (99, 2))
        self.assertNotIn(key, registry.predicted_alive)
        self.assertEqual(registry.bindings[key], (99, 2))

    def test_terminal_spawn_outcomes_cannot_conflict(self):
        registry = SpawnRegistry()
        key = ActionKey(1, 1, 3)
        registry.predict(key)
        registry.reject(key)
        with self.assertRaises(ProtocolError):
            registry.accept(key, (99, 2))

    def test_serial_wrap(self):
        self.assertTrue(serial_newer_u32(0, 0xFFFFFFFF))
        self.assertFalse(serial_newer_u32(0xFFFFFFFF, 0))
        self.assertFalse(serial_newer_u32(9, 9))
        with self.assertRaises(ProtocolError):
            serial_newer_u32(0x80000000, 0)


class RandomizedDeliveryTests(unittest.TestCase):
    def test_loss_duplication_reordering_eventual_convergence(self):
        # 100 seeds, 80 ticks, commands scheduled 4 ticks ahead. Snapshots are
        # delayed/reordered too. The final complete checkpoint is delivered.
        for seed in range(100):
            rng = random.Random(seed)
            server = Server()
            client = Client(replay_cap=32)
            input_queue = []
            snapshot_queue = []
            generated = []
            last_movement = 0
            for k in range(1, 5):
                command = Command(k, k, 0)
                client.predict(command)
                generated.append(command)
                server.submit(command)
            for wall in range(1, 81):
                target = wall + 4
                last_movement = rng.choice((-1, 0, 1))
                command = Command(target, target, last_movement, rng.random() < .2)
                client.predict(command)
                generated.append(command)
                bundle = generated[-4:]
                if rng.random() >= .15:
                    delay = rng.randrange(0, 6)
                    input_queue.append((wall + delay, bundle))
                    if rng.random() < .2:
                        input_queue.append((wall + delay + 1, bundle))
                due = [item for item in input_queue if item[0] <= wall]
                input_queue = [item for item in input_queue if item[0] > wall]
                rng.shuffle(due)
                for _, commands in due:
                    for item in commands:
                        server.submit(item)
                server.advance()
                if wall % 2 == 0 and rng.random() > .15:
                    snapshot_queue.append((wall + rng.randrange(0, 5), server.state))
                due_snapshots = [item for item in snapshot_queue if item[0] <= wall]
                snapshot_queue = [item for item in snapshot_queue if item[0] > wall]
                rng.shuffle(due_snapshots)
                for _, snapshot in due_snapshots:
                    client.reconcile(snapshot)
                self.assertLessEqual(len(server.pending), server.future_window)
            # Drain with explicit delivery of remaining commands, then a fresh
            # full checkpoint. We assert eventual authoritative convergence,
            # NOT that every lost, expired action was executed.
            for command in generated:
                server.submit(command)
            while server.state.tick < client.state.tick:
                server.advance()
            client.reconcile(server.state)
            self.assertEqual(client.state, server.state, f"seed {seed}")
            self.assertEqual(server.state.ammo, 20 - len(server.committed_actions))


if __name__ == "__main__":
    unittest.main()
