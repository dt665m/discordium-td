"""Run with: python -m unittest discover -s reference_model -v"""
import itertools
import random
import unittest
from dataclasses import replace
from math import dist
from spatial_model import (Actor, ClientScopes, ConnectionView, EntityKey, ReplicationGraph,
                           ScopeMessage, ServerScopes, SpatialError)

E = EntityKey
ORIGIN = ((0.0, 0.0, 0.0),)

class SpatialGraphTests(unittest.TestCase):
    def graph(self, *actors, **kwargs):
        g = ReplicationGraph(**kwargs)
        for a in actors: g.upsert(a)
        g.prepare(1)
        return g

    def test_two_clients_receive_distinct_spatial_sets(self):
        g = self.graph(Actor(E(1)),Actor(E(2),(1000.,0.,0.)))
        self.assertEqual(g.gather(ConnectionView(1,ORIGIN)).eligible,{E(1)})
        self.assertEqual(g.gather(ConnectionView(2,((1000.,0.,0.),))).eligible,{E(2)})

    def test_negative_cells_use_floor(self):
        g = self.graph(Actor(E(1),(-.1,-.1,0.)))
        self.assertEqual(g.cell((-.1,-.1,0.)),(-1,-1))
        self.assertIn(E(1),g.gather(ConnectionView(1,((-.05,-.05,0.),))).eligible)

    def test_footprint_changes_while_center_stays_in_cell(self):
        g = self.graph(Actor(E(1),(23.,0.,0.),cull_radius=8.),leave_margin=0.)
        c = ConnectionView(1,((33.,0.,0.),))
        self.assertNotIn(E(1),g.gather(c).eligible)
        g.upsert(replace(g.actors[E(1)],position=(25.,0.,0.)))
        g.prepare(2)
        self.assertEqual(g.cell((23.,0.,0.)),g.cell((25.,0.,0.)))
        self.assertIn(E(1),g.gather(c).eligible)

    def test_radius_change_without_center_movement(self):
        g = self.graph(Actor(E(1),cull_radius=1.))
        c = ConnectionView(1,((40.,0.,0.),))
        self.assertNotIn(E(1),g.gather(c).eligible)
        g.upsert(replace(g.actors[E(1)],cull_radius=50.));g.prepare(2)
        self.assertIn(E(1),g.gather(c).eligible)

    def test_bounds_radius_affects_influence_and_exact_test(self):
        g=self.graph(Actor(E(1),cull_radius=1.,bound_radius=40.))
        self.assertIn(E(1),g.gather(ConnectionView(1,((40.,0.,0.),))).eligible)

    def test_z_exact_filter_after_xy_candidate(self):
        g=self.graph(Actor(E(1),(0.,0.,100.)))
        result=g.gather(ConnectionView(1,ORIGIN))
        self.assertEqual(result.candidate_visits,1)
        self.assertFalse(result.eligible)

    def test_global_candidate_still_obeys_denial(self):
        g=self.graph(Actor(E(1),(1000.,0.,0.),route='global'))
        self.assertIn(E(1),g.gather(ConnectionView(1,ORIGIN)).eligible)
        self.assertFalse(g.gather(ConnectionView(1,ORIGIN,forbidden=frozenset({E(1)}))).eligible)

    def test_owner_only_does_not_spatially_leak(self):
        g=self.graph(Actor(E(1),(1000.,0.,0.),route='owner',owner=1))
        self.assertIn(E(1),g.gather(ConnectionView(1,ORIGIN)).eligible)
        self.assertFalse(g.gather(ConnectionView(2,((1000.,0.,0.),))).eligible)

    def test_team_route_and_change(self):
        g=self.graph(Actor(E(1),route='team',team=3))
        c=ConnectionView(1,ORIGIN,team=3)
        self.assertIn(E(1),g.gather(c).eligible)
        g.upsert(replace(g.actors[E(1)],team=4));g.prepare(2)
        self.assertFalse(g.gather(c).eligible)
        self.assertNotIn(3,g.teams)

    def test_multiple_observers_and_reasons_deduplicate(self):
        g=self.graph(Actor(E(1)))
        c=ConnectionView(1,ORIGIN+ORIGIN,semantic_grants=frozenset({E(1)}))
        r=g.gather(c)
        self.assertEqual(r.eligible,{E(1)});self.assertEqual(r.candidate_visits,1)
        self.assertEqual(r.reasons[E(1)],{'semantic','spatial'})

    def test_observer_limit_is_enforced(self):
        g=self.graph(Actor(E(1)))
        with self.assertRaises(SpatialError):g.gather(ConnectionView(1,ORIGIN*3))

    def test_no_replication_route_is_excluded_even_with_semantic_grant(self):
        g=self.graph(Actor(E(1),route='none'))
        self.assertFalse(g.gather(ConnectionView(1,ORIGIN,semantic_grants=frozenset({E(1)}))).eligible)

    def test_hysteresis_does_not_override_hard_denial(self):
        g=self.graph(Actor(E(1),cull_radius=8.),leave_margin=3.)
        c=ConnectionView(1,((10.,0.,0.),))
        self.assertFalse(g.gather(c).eligible)
        self.assertIn(E(1),g.gather(c,frozenset({E(1)})).eligible)
        self.assertFalse(g.gather(replace(c,forbidden=frozenset({E(1)})),frozenset({E(1)})).eligible)

    def test_required_dependency_bypasses_distance(self):
        g=self.graph(Actor(E(1),dependencies=(E(2),)),Actor(E(2),(1000.,0.,0.)))
        r=g.gather(ConnectionView(1,ORIGIN,prediction_roots=frozenset({E(1)})))
        self.assertTrue(r.prediction_admitted);self.assertEqual(r.required,{E(1),E(2)})
        self.assertIn(E(2),r.eligible)

    def test_hidden_dependency_denies_group_without_disclosure(self):
        g=self.graph(Actor(E(1),dependencies=(E(2),)),Actor(E(2),(1000.,0.,0.)))
        c=ConnectionView(1,ORIGIN,forbidden=frozenset({E(2)}),prediction_roots=frozenset({E(1)}))
        r=g.gather(c)
        self.assertFalse(r.prediction_admitted);self.assertFalse(r.required)
        self.assertNotIn(E(2),r.eligible);self.assertNotIn(E(2),r.reasons)

    def test_private_owner_dependency_cannot_bypass_owner(self):
        g=self.graph(Actor(E(1),dependencies=(E(2),)),Actor(E(2),route='owner',owner=2))
        r=g.gather(ConnectionView(1,ORIGIN,prediction_roots=frozenset({E(1)})))
        self.assertFalse(r.prediction_admitted);self.assertNotIn(E(2),r.eligible)

    def test_missing_dependency_is_not_partial_admission(self):
        g=self.graph(Actor(E(1),dependencies=(E(99),)))
        r=g.gather(ConnectionView(1,ORIGIN,prediction_roots=frozenset({E(1)})))
        self.assertFalse(r.prediction_admitted);self.assertFalse(r.required)
        self.assertEqual(r.prediction_error,'missing_dependency')

    def test_dependency_cycle_terminates(self):
        g=self.graph(Actor(E(1),dependencies=(E(2),)),Actor(E(2),dependencies=(E(1),)))
        r=g.gather(ConnectionView(1,ORIGIN,prediction_roots=frozenset({E(1)})))
        self.assertEqual(r.required,{E(1),E(2)})

    def test_dependency_capacity_rejects_entire_prediction_closure(self):
        g=self.graph(Actor(E(1),dependencies=(E(2),)),Actor(E(2),dependencies=(E(3),)),Actor(E(3)),max_required=2)
        r=g.gather(ConnectionView(1,ORIGIN,prediction_roots=frozenset({E(1)})))
        self.assertFalse(r.prediction_admitted);self.assertFalse(r.required)

    def test_candidate_limit_reports_overload_not_silent_truncation(self):
        g=self.graph(Actor(E(1),route='global'),Actor(E(2),route='global'),max_candidates=1)
        with self.assertRaises(SpatialError):g.gather(ConnectionView(1,ORIGIN))

    def test_failed_large_registration_preserves_existing_route(self):
        g=self.graph(Actor(E(1)),max_cells_per_actor=16)
        before=(dict(g.actors),dict(g.footprints),g.total_memberships)
        with self.assertRaises(SpatialError):g.upsert(replace(g.actors[E(1)],cull_radius=1e8))
        self.assertEqual(before,(g.actors,g.footprints,g.total_memberships))
        self.assertIn(E(1),g.gather(ConnectionView(1,ORIGIN)).eligible)

    def test_nonfinite_registration_rejected_before_mutation(self):
        g=self.graph(Actor(E(1)))
        with self.assertRaises(SpatialError):g.upsert(Actor(E(2),(float('nan'),0.,0.)))
        self.assertNotIn(E(2),g.actors)

    def test_teleport_removes_old_footprint(self):
        g=self.graph(Actor(E(1)))
        g.upsert(replace(g.actors[E(1)],position=(1000.,0.,0.)));g.prepare(2)
        self.assertFalse(g.gather(ConnectionView(1,ORIGIN)).eligible)
        self.assertIn(E(1),g.gather(ConnectionView(1,((1000.,0.,0.),))).eligible)

    def test_prepare_shared_once_and_changes_require_new_barrier(self):
        g=self.graph(Actor(E(1)))
        for i in range(20):g.gather(ConnectionView(i,ORIGIN))
        self.assertEqual(g.prepare_calls,1)
        with self.assertRaises(SpatialError):g.prepare(1)
        g.upsert(replace(g.actors[E(1)],dormant=True,route='dormancy'))
        with self.assertRaises(SpatialError):g.gather(ConnectionView(1,ORIGIN))
        g.prepare(2);self.assertIn(E(1),g.gather(ConnectionView(1,ORIGIN)).eligible)

    def test_removal_cleans_lists_cells_and_membership_count(self):
        g=self.graph(Actor(E(1)))
        g.remove(E(1));g.prepare(2)
        self.assertFalse(g.cells);self.assertEqual(g.total_memberships,0)
        self.assertFalse(g.gather(ConnectionView(1,ORIGIN)).eligible)

    def test_sparse_gather_visits_do_not_grow_with_distant_population(self):
        g=ReplicationGraph()
        g.upsert(Actor(E(0)))
        for i in range(1,1001):g.upsert(Actor(E(i),(1000.+100*i,0.,0.),cull_radius=1.))
        g.prepare(1);a=g.gather(ConnectionView(1,ORIGIN))
        for i in range(1001,2001):g.upsert(Actor(E(i),(1000.+100*i,0.,0.),cull_radius=1.))
        g.prepare(2);b=g.gather(ConnectionView(1,ORIGIN))
        self.assertEqual(a.eligible,b.eligible);self.assertEqual(a.candidate_visits,1)
        self.assertEqual(b.candidate_visits,1)

    def test_randomized_spatial_oracle_25_seeds_20_queries(self):
        for seed in range(25):
            rng=random.Random(seed);g=ReplicationGraph();actors={}
            for i in range(200):
                a=Actor(E(i),tuple(rng.uniform(-200,200) for _ in range(3)),
                        route=rng.choice(['dynamic','static','dormancy','global','owner','team','none']),
                        cull_radius=rng.uniform(1,30),bound_radius=rng.uniform(0,3),
                        owner=rng.choice([1,2]),team=rng.choice([1,2]))
                actors[a.key]=a;g.upsert(a)
            previous=frozenset()
            for q in range(20):
                for _ in range(3):
                    key=rng.choice(list(actors));a=replace(actors[key],position=tuple(rng.uniform(-200,200) for _ in range(3)))
                    actors[key]=a;g.upsert(a)
                g.prepare(q+1)
                c=ConnectionView(1,tuple(tuple(rng.uniform(-200,200) for _ in range(3)) for _ in range(2)),team=1,
                    forbidden=frozenset(rng.sample(list(actors),5)))
                expected=set()
                # Independent full scan: this test oracle never runs in gather().
                for key,a in actors.items():
                    if a.route=='none' or key in c.forbidden:continue
                    if a.route=='owner' and a.owner!=c.connection:continue
                    if a.route=='team' and a.team!=c.team:continue
                    radius=a.cull_radius+a.bound_radius+(g.leave_margin if key in previous else 0.)
                    if a.route in ('global','owner','team') or any(dist(p,a.position)<=radius for p in c.observers):
                        expected.add(key)
                result=g.gather(c,previous)
                self.assertEqual(result.eligible,expected,(seed,q))
                previous=result.eligible

class ScopeContractTests(unittest.TestCase):
    def test_full_entry_then_delta_requires_exact_ready_ack(self):
        s=ServerScopes();c=ClientScopes();entry=s.enter(E(1))
        self.assertEqual(s.pending_state(E(1)).kind,'enter')
        self.assertTrue(c.apply(entry));self.assertTrue(s.acknowledge_state(entry))
        s.changed(E(1),2);self.assertEqual(s.pending_state(E(1)).kind,'delta')

    def test_missing_full_entry_rejects_delta_without_allocating_replica(self):
        s=ServerScopes();c=ClientScopes();entry=s.enter(E(1))
        self.assertFalse(c.apply(replace(entry,kind='delta')))
        self.assertFalse(c.current)

    def test_old_scope_ack_cannot_ready_new_scope(self):
        s=ServerScopes();old=s.enter(E(1));s.exit(E(1));new=s.enter(E(1))
        self.assertNotEqual(old.token,new.token)
        self.assertFalse(s.acknowledge_state(old));self.assertFalse(s.current[E(1)].ready)

    def test_late_exit_does_not_delete_reentered_scope(self):
        s=ServerScopes();c=ClientScopes();old=s.enter(E(1));c.apply(old)
        exit_old=s.exit(E(1));new=s.enter(E(1));c.apply(new)
        self.assertTrue(c.apply(exit_old));self.assertEqual(c.current[E(1)][0],new.token)

    def test_exit_overtaking_entry_prevents_resurrection(self):
        s=ServerScopes();c=ClientScopes();entry=s.enter(E(1));exit_msg=s.exit(E(1))
        self.assertTrue(c.apply(exit_msg));self.assertFalse(c.apply(entry));self.assertFalse(c.current)

    def test_lost_exit_can_be_retried_idempotently(self):
        s=ServerScopes();c=ClientScopes();c.apply(s.enter(E(1)))
        lost=s.exit(E(1));retry=s.exit(E(1));self.assertEqual(lost,retry)
        c.apply(retry);self.assertTrue(s.acknowledge_exit(retry))
        self.assertFalse(c.current);self.assertFalse(s.current)

    def test_old_exit_ack_cannot_remove_new_server_scope(self):
        s=ServerScopes();s.enter(E(1));exit_msg=s.exit(E(1));new=s.enter(E(1))
        self.assertFalse(s.acknowledge_exit(exit_msg));self.assertEqual(s.current[E(1)].token,new.token)

    def test_representation_change_uses_new_full_scope(self):
        s=ServerScopes();c=ClientScopes();old=s.enter(E(1),representation=1);c.apply(old);s.acknowledge_state(old)
        new=s.enter(E(1),representation=2);self.assertEqual(new.kind,'enter');c.apply(new)
        self.assertFalse(c.apply(replace(old,kind='delta',state_version=99)))
        self.assertEqual(c.current[E(1)],(new.token,new.state_version))

    def test_same_epoch_different_representation_is_rejected(self):
        s=ServerScopes();c=ClientScopes();msg=s.enter(E(1));c.apply(msg)
        bad=replace(msg,token=replace(msg.token,representation_revision=2))
        self.assertFalse(c.apply(bad))

    def test_old_state_version_does_not_regress_client(self):
        s=ServerScopes();c=ClientScopes();entry=s.enter(E(1),version=7);c.apply(entry);s.acknowledge_state(entry)
        s.changed(E(1),8);new=s.pending_state(E(1));c.apply(new);c.apply(entry)
        self.assertEqual(c.current[E(1)][1],8)

    def test_destroyed_generation_stays_dead_but_new_generation_can_enter(self):
        s=ServerScopes();c=ClientScopes();old=s.enter(E(1));c.apply(old)
        c.apply(replace(old,kind='destroy'));self.assertFalse(c.apply(old))
        new=s.enter(E(1,2));self.assertTrue(c.apply(new));self.assertIn(E(1,2),c.current)

    def test_connection_reset_rejects_old_scope_traffic(self):
        s=ServerScopes();c=ClientScopes();old=s.enter(E(1));c.apply(old)
        s.reset(2);c.reset(2)
        self.assertFalse(c.apply(old));self.assertFalse(s.acknowledge_state(old))
        self.assertTrue(c.apply(s.enter(E(1))))

    def test_dormancy_completion_is_per_connection(self):
        a=ServerScopes();b=ServerScopes();ma=a.enter(E(1),7,dormant=True);mb=b.enter(E(1),6,dormant=True)
        a.acknowledge_state(ma);b.acknowledge_state(mb);b.changed(E(1),7)
        self.assertFalse(a.needs_state(E(1)));self.assertTrue(b.needs_state(E(1)))
        self.assertEqual(a.current[E(1)].phase,'dormant_known');self.assertEqual(b.current[E(1)].phase,'active')

    def test_dropped_wake_state_retries_until_decode(self):
        s=ServerScopes();entry=s.enter(E(1),7,dormant=True);s.acknowledge_state(entry)
        s.changed(E(1),8);lost=s.pending_state(E(1));retry=s.pending_state(E(1))
        self.assertEqual(lost,retry);self.assertTrue(s.needs_state(E(1)))
        s.acknowledge_state(retry);self.assertFalse(s.needs_state(E(1)))

    def test_late_join_to_dormant_actor_gets_full_state(self):
        s=ServerScopes();entry=s.enter(E(1),7,dormant=True)
        self.assertEqual(entry.kind,'enter');self.assertTrue(s.needs_state(E(1)))
        s.acknowledge_state(entry);self.assertEqual(s.current[E(1)].phase,'dormant_known')

    def test_unsent_version_ack_cannot_complete_dormancy(self):
        s=ServerScopes();entry=s.enter(E(1),7,dormant=True);s.acknowledge_state(entry);s.changed(E(1),8)
        self.assertFalse(s.acknowledge_state(replace(entry,kind='delta',state_version=8)))
        self.assertTrue(s.needs_state(E(1)))

    def test_old_ack_does_not_clear_new_pending_version(self):
        s=ServerScopes();entry=s.enter(E(1),7,dormant=True);s.acknowledge_state(entry);s.changed(E(1),8)
        s.acknowledge_state(entry);self.assertTrue(s.needs_state(E(1)))
        self.assertEqual(s.current[E(1)].phase,'active')

    def test_identity_counter_cap_requires_explicit_recovery(self):
        s=ServerScopes(max_identities=1);m=s.enter(E(1));s.acknowledge_exit(s.exit(E(1)))
        with self.assertRaises(SpatialError):s.enter(E(2))
        self.assertGreater(s.enter(E(1)).token.scope_epoch,m.token.scope_epoch)

    def test_client_scope_cap_is_not_unbounded(self):
        s=ServerScopes();c=ClientScopes(max_identities=1);c.apply(s.enter(E(1)))
        with self.assertRaises(SpatialError):c.apply(s.enter(E(2)))

    def test_all_120_delivery_orders_respect_scope_retirement(self):
        s=ServerScopes();e1=s.enter(E(1));x1=s.exit(E(1));e2=s.enter(E(1));s.acknowledge_state(e2)
        s.changed(E(1),2);d2=s.pending_state(E(1));x2=s.exit(E(1))
        for order in itertools.permutations((e1,x1,e2,d2,x2)):
            c=ClientScopes()
            for msg in order:c.apply(msg)
            self.assertFalse(c.current)
            self.assertFalse(c.apply(e2))

if __name__ == '__main__':
    unittest.main()
