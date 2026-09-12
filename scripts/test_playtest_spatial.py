"""Synthetic trace checks only; these tests are not real-process qualification."""
import copy
import binascii
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
import zlib

spec = importlib.util.spec_from_file_location("spatial_playtest", Path(__file__).with_name("playtest-spatial.py"))
spatial = importlib.util.module_from_spec(spec)
spec.loader.exec_module(spatial)


def scope(observer, remote, incarnation=1):
    return {"connection": observer + 100, "entity": {"index": remote, "generation": 1},
            "scope": incarnation, "representation": 1}


def record(side, tick, separated, incarnation=1):
    owner = spatial.PLAYERS[side]
    remote = spatial.PLAYERS[1-side]
    current = scope(side, remote, incarnation)
    distance = {1450: 40, 1465: 60, 1480: 80}.get(tick, 80)
    position = [(-1 if side == 0 else 1) * (distance if separated else 1), 0 if separated else 2]
    return {"format": 1, "line": tick, "pid": 101 + side, "elapsed": tick / 60,
            "owner": owner, "match_epoch": 1, "server_tick": tick,
            "decode_errors": 0, "stale_epoch_rejections": 0, "clock_resync_rejections": 0,
            "obsolete_rejections": 0, "decode_failures": 0, "decode_issue": None,
            "phase": "Combat", "active": True, "separated": separated,
            "position": position.copy(), "authoritative_owner_position": position.copy(),
            "owner_checkpoint_tick": tick,
            "heroes": [owner] if separated else list(spatial.PLAYERS),
            "scopes": [] if separated else [current],
            "hero_scopes": [] if separated else [{"hero_id": remote, "scope": current, "end_tick": tick,
                                                       "position": [-position[0], position[1]]}]}


def traces():
    return [[record(side, tick, separated, incarnation)
             for tick, separated, incarnation in [(800, False, 1), (1450, True, 1),
                (1465, True, 1), (1480, True, 1), (2200, False, 2)]] for side in (0, 1)]


class TraceAnalysis(unittest.TestCase):
    def test_lifetime_hard_failure_cannot_be_hidden_by_latest_benign_issue(self):
        left, right = traces()
        for index, row in enumerate(right):
            if index >= 1:
                row.update(decode_errors=1, decode_failures=1, decode_issue="MalformedClock")
            if index >= 2:
                row.update(decode_errors=22, obsolete_rejections=21, decode_issue="Obsolete")
        result = spatial.analyze_records(left, right)
        self.assertFalse(result["passed"])
        self.assertTrue(all(result["reentries"]))
        health = result["metrics"][1]["decode_health"]
        self.assertTrue(health["classification_verified"])
        self.assertEqual(health["observed_maxima"]["decode_failures"], 1)
        self.assertEqual(health["first_issues"]["hard_failure"]["line"], right[1]["line"])
        self.assertIn("player 1002: lifetime hard decode_failures reached 1", "\n".join(result["failures"]))

    def test_benign_clock_stale_epoch_and_obsolete_counters_remain_distinct(self):
        left, right = traces()
        startup = record(1, 600, False)
        startup.update(active=False, decode_errors=1, clock_resync_rejections=1, decode_issue="Clock(ResyncRequested)")
        right.insert(0, startup)
        for index, row in enumerate(right[1:]):
            row.update(decode_errors=3 + index, clock_resync_rejections=1, stale_epoch_rejections=2,
                       obsolete_rejections=index, decode_issue="Obsolete" if index else "WrongEpoch")
        result = spatial.analyze_records(left, right)
        self.assertTrue(result["passed"], result["failures"])
        health = result["metrics"][1]["decode_health"]
        self.assertTrue(health["classification_verified"])
        self.assertEqual(health["observed_maxima"], {"decode_errors": 7, "stale_epoch_rejections": 2,
                         "clock_resync_rejections": 1, "obsolete_rejections": 4, "decode_failures": 0})

    def test_inconsistent_decode_partition_fails_even_with_zero_hard_counter(self):
        for total in (2, 4):
            with self.subTest(total=total):
                left, right = traces()
                for row in right:
                    row.update(decode_errors=total, obsolete_rejections=3, decode_issue="Obsolete")
                result = spatial.analyze_records(left, right)
                self.assertFalse(result["passed"])
                health = result["metrics"][1]["decode_health"]
                self.assertFalse(health["classification_verified"])
                self.assertEqual(health["first_issues"]["partition_mismatch"]["classified_total"], 3)
                self.assertIn("decode counter partition is inconsistent", "\n".join(result["failures"]))

    def test_legacy_missing_or_invalid_decode_classification_is_unverifiable(self):
        for field in spatial.DECODE_COUNTERS:
            for value in (None, -1, True, 0.0):
                with self.subTest(field=field, value=value):
                    left, right = traces()
                    right[1][field] = value
                    result = spatial.analyze_records(left, right)
                    self.assertFalse(result["passed"])
                    self.assertIn("classification is unverifiable", "\n".join(result["failures"]))
        left, right = traces()
        for row in right:
            for field in spatial.DECODE_COUNTERS:
                row.pop(field)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "legacy.ndjson"
            path.write_text("".join(json.dumps(row) + "\n" for row in right))
            rows, errors = spatial.read_trace(path)
        self.assertFalse(errors, "legacy traces remain readable for historical analysis")
        result = spatial.analyze_records(left, rows)
        self.assertFalse(result["passed"])
        self.assertFalse(result["metrics"][1]["decode_health"]["classification_verified"])

    def test_inactive_hard_failure_and_lifetime_counter_reset_both_fail(self):
        left, right = traces()
        startup = record(1, 600, False)
        startup.update(active=False, decode_errors=1, decode_failures=1, decode_issue="MalformedClock")
        right.insert(0, startup)
        result = spatial.analyze_records(left, right)
        self.assertFalse(result["passed"])
        health = result["metrics"][1]["decode_health"]
        self.assertEqual(set(health["first_issues"]), {"hard_failure", "counter_regression"})
        self.assertFalse(health["classification_verified"])
        self.assertIn("lifetime decode counters regressed", "\n".join(result["failures"]))

    def test_pause_recovery_requires_same_process_server_and_new_active_stream(self):
        before = record(0, 800, False)
        before.update(connection_epoch=7, server_instance=123)
        after = record(0, 1000, False)
        after.update(connection_epoch=8, server_instance=123)
        self.assertTrue(spatial.recovered_after_pause(before, after))
        for changes in ({"active": False}, {"connection_epoch": 7},
                        {"connection_epoch": None}, {"pid": 102},
                        {"server_instance": 456}, {"owner_checkpoint_tick": 800},
                        {"authoritative_owner_position": None}, {"elapsed": 0}):
            self.assertFalse(spatial.recovered_after_pause(before, {**after, **changes}), changes)
        self.assertFalse(spatial.recovered_after_pause(before, None))
        self.assertFalse(spatial.recovered_after_pause({**before, "active": False}, after))

    def test_both_directions_require_omit_then_same_entity_new_scope(self):
        result = spatial.analyze_records(*traces(), [101, 102])
        self.assertTrue(result["passed"], result["failures"])
        self.assertEqual(result["eligible_separated_observations"], 3)
        self.assertEqual(len([proof for proof in result["reentries"] if proof]), 2)

    def test_waypoint_intent_cannot_override_authoritative_separation_or_return(self):
        left, right = traces()
        for rows in (left, right):
            for row in rows:
                row["separated"] = not row["separated"]
        result = spatial.analyze_records(left, right, [101, 102])
        self.assertTrue(result["passed"], result["failures"])
        self.assertEqual(result["eligible_separated_observations"], 3)
        self.assertTrue(all(result["reentries"]))

    def test_early_scope_proofs_cannot_hide_disconnected_remainder(self):
        left, right = traces()
        for side, rows in enumerate((left, right)):
            for tick in (2400, 4800, 7200):
                row = record(side, tick, False, 2)
                row["active"] = False
                rows.append(row)
        result = spatial.analyze_records(left, right, duration=120)
        self.assertFalse(result["passed"])
        self.assertTrue(all(result["reentries"]))
        for metrics in result["metrics"]:
            continuity = metrics["connection_continuity"]
            self.assertEqual(continuity["longest_inactive_after_activation_seconds"], 80)
            self.assertFalse(continuity["active_near_run_end"])
            self.assertLess(continuity["active_fraction"], 0.25)

    def test_separation_inside_two_cells_does_not_qualify_large_world_traversal(self):
        left, right = traces()
        for side, rows in enumerate((left, right)):
            for row in rows:
                if row["separated"]:
                    row["authoritative_owner_position"] = [-14 if side == 0 else 14, 0]
        result = spatial.analyze_records(left, right)
        self.assertFalse(result["passed"])
        self.assertEqual(result["eligible_separated_observations"], 3)
        self.assertTrue(all(result["reentries"]))
        self.assertTrue(any("graph cells" in failure for failure in result["failures"]))

    def test_short_recovery_then_active_run_end_is_allowed(self):
        left, right = traces()
        for side, rows in enumerate((left, right)):
            for tick, active in ((2400, False), (2700, True), (7200, True)):
                row = record(side, tick, False, 2)
                row["active"] = active
                rows.append(row)
        result = spatial.analyze_records(left, right, duration=120)
        self.assertTrue(result["passed"], result["failures"])
        for metrics in result["metrics"]:
            self.assertEqual(metrics["connection_continuity"]["longest_inactive_after_activation_seconds"], 5)
            self.assertTrue(metrics["connection_continuity"]["active_near_run_end"])

    def test_long_recovery_fails_even_if_active_at_end(self):
        left, right = traces()
        for side, rows in enumerate((left, right)):
            for tick, active in ((2400, False), (3060, True), (7200, True)):
                row = record(side, tick, False, 2)
                row["active"] = active
                rows.append(row)
        result = spatial.analyze_records(left, right, duration=120)
        self.assertFalse(result["passed"])
        self.assertTrue(any("recovery deadline" in failure for failure in result["failures"]))

    def test_no_eligible_observation_is_a_failure(self):
        left, right = traces()
        for own, other in zip(right, left):
            own["authoritative_owner_position"] = other["authoritative_owner_position"].copy()
        result = spatial.analyze_records(left, right)
        self.assertFalse(result["passed"])
        self.assertEqual(result["eligible_separated_observations"], 0)

    def test_no_inferred_mapping_or_reconnect_can_prove_reentry(self):
        for mutation in ("missing", "same_scope", "new_connection", "new_entity", "not_live"):
            left, right = traces()
            row = left[-1]
            if mutation == "missing":
                row.pop("hero_scopes")
            elif mutation == "same_scope":
                row["scopes"][0]["scope"] = 1
            elif mutation == "new_connection":
                row["scopes"][0]["connection"] += 1
            elif mutation == "new_entity":
                row["scopes"][0]["entity"]["generation"] += 1
            else:
                row["scopes"] = []
            result = spatial.analyze_records(left, right)
            self.assertFalse(result["passed"], mutation)
            self.assertIsNone(result["reentries"][0], mutation)

    def test_disclosure_after_separation_fails_even_if_reentry_later_succeeds(self):
        left, right = traces()
        left[2]["heroes"].append(1002)
        current = scope(0, 1002)
        left[2]["scopes"].append(current)
        left[2]["hero_scopes"].append({"hero_id": 1002, "scope": current,
                                      "end_tick": 1465, "position": [14, 0]})
        result = spatial.analyze_records(left, right)
        self.assertFalse(result["passed"])
        self.assertEqual(len(result["disclosure_violations"]), 1)

    def test_predicted_overshoot_is_not_authoritative_separation(self):
        left, right = traces()
        for rows, side in ((left, 0), (right, 1)):
            for row in rows[1:4]:
                row["authoritative_owner_position"] = [-1 if side == 0 else 1, 2]
                row["position"] = [-40 if side == 0 else 40, 0]
        result = spatial.analyze_records(left, right)
        self.assertFalse(result["passed"])
        self.assertEqual(result["eligible_separated_observations"], 0)
        self.assertEqual(result["disclosure_violations"], [])

    def test_stale_retained_replica_is_not_a_fresh_disclosure(self):
        left, right = traces()
        left[1]["heroes"].append(1002)
        left[1]["scopes"] = copy.deepcopy(left[0]["scopes"])
        left[1]["hero_scopes"] = copy.deepcopy(left[0]["hero_scopes"])
        result = spatial.analyze_records(left, right)
        self.assertTrue(result["passed"], result["failures"])
        self.assertEqual(result["disclosure_violations"], [])
        self.assertEqual(len(result["retained_remote_examples"]), 1)
        # Retention forever still cannot prove omission or reentry.
        for row in left[1:4]:
            row["heroes"] = list(spatial.PLAYERS)
            row["scopes"] = copy.deepcopy(left[0]["scopes"])
            row["hero_scopes"] = copy.deepcopy(left[0]["hero_scopes"])
        self.assertFalse(spatial.analyze_records(left, right)["passed"])

    def test_delayed_fresh_far_update_is_still_a_disclosure(self):
        left, right = traces()
        # Received at return time, but the payload reveals a known far tick.
        left[-1]["hero_scopes"][0]["end_tick"] = 1465
        result = spatial.analyze_records(left, right)
        self.assertFalse(result["passed"])
        self.assertEqual(len(result["disclosure_violations"]), 1)

    def test_missing_authoritative_pose_never_falls_back_to_prediction(self):
        left, right = traces()
        for row in right:
            row.pop("authoritative_owner_position")
        result = spatial.analyze_records(left, right)
        self.assertFalse(result["passed"])
        self.assertEqual(result["paired_observations"], 0)

    def test_different_epoch_or_distant_ticks_do_not_pair(self):
        for epoch in (False, True):
            left, right = traces()
            for row in right[1:4]:
                if epoch:
                    row["match_epoch"] = 2
                else:
                    row["owner_checkpoint_tick"] += 1
            result = spatial.analyze_records(left, right)
            self.assertFalse(result["passed"])
            self.assertEqual(result["eligible_separated_observations"], 0)

    def test_scope_must_really_leave_the_live_set(self):
        left, right = traces()
        for row in left[1:4]:
            row["scopes"] = copy.deepcopy(left[0]["scopes"])
        result = spatial.analyze_records(left, right)
        self.assertFalse(result["passed"])
        self.assertIsNone(result["reentries"][0])

    def test_pid_and_owner_identity_are_checked(self):
        left, right = traces()
        self.assertFalse(spatial.analyze_records(left, right, [999, 102])["passed"])
        for row in right:
            row["pid"] = 101
        self.assertFalse(spatial.analyze_records(left, right)["passed"])
        right[0]["owner"] = 5000
        self.assertTrue(any("identity changed" in text for text in spatial.analyze_records(left, right)["failures"]))

    def test_malformed_or_truncated_trace_cannot_be_silently_accepted(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "trace.ndjson"
            row = record(0, 800, False)
            path.write_text(json.dumps(row) + "\n{partial")
            records, errors = spatial.read_trace(path)
            self.assertEqual(len(records), 1)
            self.assertTrue(errors)
            row["position"][0] = float("nan")
            path.write_text(json.dumps(row) + "\n")
            self.assertFalse(spatial.read_trace(path)[0])

    def test_opaque_black_png_is_not_rendered_color_evidence(self):
        def chunk(kind, body):
            return len(body).to_bytes(4, "big") + kind + body + binascii.crc32(kind + body).to_bytes(4, "big")
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "capture.png"
            header = (32).to_bytes(4, "big") * 2 + bytes([8, 6, 0, 0, 0])
            for color, expected in ((0, False), (20, True)):
                pixels = (b"\0" + bytes([color, 0, 0, 255]) * 32) * 32
                path.write_bytes(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header)
                                 + chunk(b"tEXt", b"test\0" + b"padding" * 200)
                                 + chunk(b"IDAT", zlib.compress(pixels)) + chunk(b"IEND", b""))
                self.assertEqual(spatial.capture_has_color(path), expected)


class FrozenRunner(unittest.TestCase):
    def fixture(self, directory):
        import shutil
        base = directory / "sources"
        workspace = base / "discordium-td"
        (workspace / "scripts").mkdir(parents=True)
        (workspace / "Cargo.toml").write_text("[workspace]\n")
        shutil.copyfile(Path(__file__).with_name("qualify-soak.py"),workspace / "scripts/qualify-soak.py")
        manifest = directory / "source-manifest.json"
        entries = [{"path":str(p.relative_to(base)),"sha256":spatial.digest(p)}
                   for p in base.rglob("*") if p.is_file()]
        manifest.write_text(json.dumps({"files":entries}))
        binaries = directory / "binaries"
        binaries.mkdir()
        for name in ("game_server","dreamwake"):
            (binaries/name).write_bytes(name.encode())
            (binaries/name).chmod(0o700)
        (directory / "native-provenance.json").write_text(json.dumps({
            "source_manifest_sha256":spatial.digest(manifest),
            "target": "native", "profiles": {name: "release" for name in ("game_server", "dreamwake")},
            "artifacts":{name:{"sha256":spatial.digest(binaries/name), "profile": {"opt_level": "3", "test": False}}
                         for name in ("game_server","dreamwake")}}))
        return workspace,manifest,binaries

    def run_fake(self, directory, mutate=None):
        from unittest.mock import patch
        workspace,manifest,binaries=self.fixture(directory)
        output=directory/"outside-output"
        args=["playtest-spatial.py","--no-build","--source-manifest",str(manifest),
              "--binary-dir",str(binaries),"--output-root",str(output)]
        def run_profile(*values):
            if mutate:mutate(workspace,manifest,binaries,values[4])
            return {"errors":[]}
        with patch.object(spatial,"ROOT",workspace),patch.object(spatial.sys,"argv",args), \
             patch.object(spatial,"run_profile",side_effect=run_profile) as launched, \
             patch.object(spatial,"analyze_profile",return_value={"passed":True}), \
             patch.object(spatial.platform,"platform",return_value="test-platform"), \
             patch.object(spatial.subprocess,"check_output",side_effect=AssertionError("Git must not run")):
            result=spatial.main()
        self.assertEqual(launched.call_count,1)
        run=next(output.iterdir())
        return result,json.loads((run/"manifest.json").read_text()),json.loads((run/"summary.json").read_text())

    def test_frozen_external_output_records_identity_without_git(self):
        with tempfile.TemporaryDirectory() as tmp:
            result,manifest,summary=self.run_fake(Path(tmp))
            self.assertEqual(result,0)
            self.assertIsNone(manifest["git_head"])
            self.assertTrue(summary["inputs_unchanged_after"])
            self.assertEqual(summary["source_manifest_sha256"],manifest["frozen"]["source_manifest_sha256"])

    def test_frozen_run_detects_changed_source_manifest_original_or_captured_binary(self):
        mutations=[lambda w,m,b,c:(w/"Cargo.toml").write_text("changed"),
                   lambda w,m,b,c:m.write_text("{}"),
                   lambda w,m,b,c:(b/"game_server").write_bytes(b"changed"),
                   lambda w,m,b,c:(c[0].chmod(0o700),c[0].write_bytes(b"changed"))]
        for mutate in mutations:
            with self.subTest(mutate=mutate),tempfile.TemporaryDirectory() as tmp:
                result,_,summary=self.run_fake(Path(tmp),mutate)
                self.assertEqual(result,1)
                self.assertFalse(summary["inputs_unchanged_after"])

    def test_wrong_binary_provenance_and_live_external_output_reject_before_launch(self):
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as tmp:
            workspace,manifest,binaries=self.fixture(Path(tmp))
            provenance=Path(tmp)/"native-provenance.json"
            data=json.loads(provenance.read_text());data["source_manifest_sha256"]="0"*64
            provenance.write_text(json.dumps(data))
            for frozen in [True,False]:
                args=["playtest-spatial.py","--no-build","--binary-dir",str(binaries),"--output-root",str(Path(tmp)/"external")]
                if frozen:args += ["--source-manifest",str(manifest)]
                with patch.object(spatial,"ROOT",workspace),patch.object(spatial.sys,"argv",args), \
                     patch.object(spatial,"run_profile") as launched,self.assertRaises(SystemExit):
                    spatial.main()
                launched.assert_not_called()

    def test_profile_evidence_rejects_before_any_process(self):
        from unittest.mock import patch
        mutations = [lambda p: p.pop("profiles"),
                     lambda p: p.update(profiles={"game_server": "dev", "dreamwake": "dev"}),
                     lambda p: p["profiles"].update(dreamwake="dev"),
                     lambda p: p.pop("target"),
                     lambda p: p["artifacts"]["dreamwake"].pop("profile"),
                     lambda p: p["artifacts"]["dreamwake"]["profile"].update(opt_level="0")]
        for mutation in mutations:
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as tmp:
                directory = Path(tmp)
                workspace, manifest, binaries = self.fixture(directory)
                provenance = directory / "native-provenance.json"
                value = json.loads(provenance.read_text())
                mutation(value)
                provenance.write_text(json.dumps(value))
                args = ["playtest-spatial.py", "--no-build", "--source-manifest", str(manifest),
                        "--binary-dir", str(binaries), "--output-root", str(directory / "runs")]
                with patch.object(spatial, "ROOT", workspace), patch.object(spatial.sys, "argv", args), \
                     patch.object(spatial, "run_profile") as launched, \
                     patch.object(spatial.subprocess, "run") as process, self.assertRaises(SystemExit):
                    spatial.main()
                launched.assert_not_called()
                process.assert_not_called()

    def test_historical_analysis_accepts_missing_build_profiles_without_launch(self):
        from argparse import Namespace
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp).resolve()
            workspace, source_manifest, binaries = self.fixture(directory)
            provenance = directory / "native-provenance.json"
            value = json.loads(provenance.read_text())
            value.pop("profiles")
            for item in value["artifacts"].values():
                item.pop("profile")
            provenance.write_text(json.dumps(value))
            output = directory / "historical-run"
            output.mkdir()
            args = Namespace(no_build=True, binary_dir=binaries, source_manifest=source_manifest,
                             source_root=None, binary_provenance=None, analyze=output)
            with patch.object(spatial, "ROOT", workspace):
                identity, _ = spatial.frozen_context(args, output)
            _, captured = spatial.capture_binaries([binaries / name for name in ("game_server", "dreamwake")], output)
            spatial.save_json(output / "manifest.json", {"frozen": identity, "profiles": ["lan"], "binaries": captured})
            (output / "lan").mkdir()
            spatial.save_json(output / "lan/processes.json", {"errors": []})
            argv = ["playtest-spatial.py", "--no-build", "--source-manifest", str(source_manifest),
                    "--binary-dir", str(binaries), "--analyze", str(output)]
            with patch.object(spatial, "ROOT", workspace), patch.object(spatial.sys, "argv", argv), \
                 patch.object(spatial, "analyze_profile", return_value={"passed": True}), \
                 patch.object(spatial, "run_profile") as launched, patch.object(spatial.subprocess, "run") as process:
                self.assertEqual(spatial.main(), 0)
            launched.assert_not_called()
            process.assert_not_called()


class LiveBuildRunner(unittest.TestCase):
    def test_release_build_uses_emitted_paths_and_preserves_profile(self):
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as tmp:
            workspace = Path(tmp).resolve()
            artifacts = {}
            for name in ("game_server", "dreamwake"):
                source = workspace / f"emitted-{name}"
                source.write_bytes(name.encode())
                source.chmod(0o700)
                artifacts[name] = {"source_path": str(source), "sha256": spatial.digest(source),
                                   "profile": {"opt_level": "3", "test": False}}
                stale = workspace / "target/release" / name
                stale.parent.mkdir(parents=True, exist_ok=True)
                stale.write_bytes(b"STALE: do not capture")
            build = {"target": "native", "profiles": {name: "release" for name in artifacts}, "artifacts": artifacts}
            def run_profile(*values):
                self.assertEqual([path.read_bytes() for path in values[4]], [b"game_server", b"dreamwake"])
                return {"errors": []}
            argv = ["playtest-spatial.py", "--output-root", str(workspace / "out/runs")]
            with patch.object(spatial, "ROOT", workspace), patch.object(spatial.sys, "argv", argv), \
                 patch.dict(spatial.NATIVE_HELPERS, {"build_pair": lambda root, out: build}), \
                 patch.object(spatial, "run_profile", side_effect=run_profile), \
                 patch.object(spatial, "analyze_profile", return_value={"passed": True}), \
                 patch.object(spatial.subprocess, "check_output", return_value="test-head"), \
                 patch.object(spatial.platform, "platform", return_value="test-host"):
                self.assertEqual(spatial.main(), 0)
            manifest = json.loads(next((workspace / "out/runs").glob("*/manifest.json")).read_text())
            self.assertIn("--release", manifest["build_command"])
            self.assertIn("--message-format=json-render-diagnostics", manifest["build_command"])
            self.assertEqual([entry["profile"]["opt_level"] for entry in manifest["binaries"]], ["3", "3"])
            prior = next((workspace / "out/runs").iterdir())
            provenance = json.loads((prior / "native-provenance.json").read_text())
            self.assertEqual(provenance["artifacts"]["dreamwake"]["path"], "binaries/dreamwake")
            # Reuse depends only on immutable copies, even after Cargo output disappears.
            for artifact in artifacts.values():
                Path(artifact["source_path"]).unlink()
            reuse = ["playtest-spatial.py", "--no-build", "--binary-dir", str(prior / "binaries"),
                     "--output-root", str(workspace / "out/reuse")]
            with patch.object(spatial, "ROOT", workspace), patch.object(spatial.sys, "argv", reuse), \
                 patch.object(spatial, "run_profile", side_effect=run_profile) as launched, \
                 patch.object(spatial, "analyze_profile", return_value={"passed": True}), \
                 patch.object(spatial.subprocess, "check_output", return_value="test-head"), \
                 patch.object(spatial.platform, "platform", return_value="test-host"), \
                 patch.object(spatial.subprocess, "run", side_effect=AssertionError("reuse must not build")):
                self.assertEqual(spatial.main(), 0)
                self.assertEqual(launched.call_count, 1)
            copied = next((workspace / "out/reuse").glob("*/native-provenance.json"))
            self.assertEqual(json.loads(copied.read_text())["artifacts"], provenance["artifacts"])

    def test_no_build_defaults_to_release_and_requires_provenance(self):
        from unittest.mock import patch
        for with_provenance in (False, True):
            with self.subTest(with_provenance=with_provenance), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp).resolve()
                binaries = root / "target/release"
                binaries.mkdir(parents=True)
                artifacts = {}
                for name in ("game_server", "dreamwake"):
                    path = binaries / name
                    path.write_bytes(name.encode())
                    path.chmod(0o700)
                    artifacts[name] = {"sha256": spatial.digest(path), "profile": {"opt_level": "3", "test": False}}
                if with_provenance:
                    spatial.save_json(root / "target/native-provenance.json", {
                        "target": "native", "profiles": {name: "release" for name in artifacts}, "artifacts": artifacts})
                argv = ["playtest-spatial.py", "--no-build", "--output-root", str(root / "out/runs")]
                with patch.object(spatial, "ROOT", root), patch.object(spatial.sys, "argv", argv), \
                     patch.object(spatial, "run_profile", return_value={"errors": []}) as launched, \
                     patch.object(spatial, "analyze_profile", return_value={"passed": True}), \
                     patch.object(spatial.subprocess, "check_output", return_value="test-head"), \
                     patch.object(spatial.platform, "platform", return_value="test-host"), \
                     patch.object(spatial.subprocess, "run") as process:
                    if with_provenance:
                        self.assertEqual(spatial.main(), 0)
                        self.assertEqual(launched.call_count, 1)
                    else:
                        with self.assertRaises(SystemExit):
                            spatial.main()
                        launched.assert_not_called()
                    process.assert_not_called()


if __name__ == "__main__":
    unittest.main()
