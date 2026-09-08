import copy
import importlib.util
from pathlib import Path
import unittest
spec = importlib.util.spec_from_file_location("compare_eval", Path(__file__).with_name("compare-netcode-eval.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


def report():
    result = {"measured_ticks": 120, "offered_updates": 120, "received_updates": 120,
              "recovered_clients": 1, "max_final_tick_lag": 0}
    for path in module.METRICS.values():
        if len(path) == 1:
            result[path[0]] = 1 if "ratio" in path[0] else 100
        else:
            result.setdefault(path[0], {})[path[1]] = 100
    cases = [{"players": 1, "entities": 84, "condition": condition, "repetition": 0,
              "workload_signature": "abc", "result": copy.deepcopy(result)} for condition in ["Clean", "MixedLoss"]]
    return {"schema_version": 2, "profile": "release", "target": "test", "ticks": 180, "repetitions": 1,
            "players": [1], "entities": [84], "seed": 7, "warmup_ticks": 60, "cases": cases}


class CompareTests(unittest.TestCase):
    def test_measures_improvement(self):
        before, after = report(), report()
        after["cases"][0]["result"]["mean_payload_bytes"] = 70
        self.assertAlmostEqual(module.compare(before, after)[0]["metrics"]["bytes_per_update"]["change_percent"], -30)

    def test_rejects_changed_workload_and_missing_duplicate_or_empty_cases(self):
        before = report()
        variants = [report() for _ in range(5)]
        variants[0]["cases"][0]["workload_signature"] = "different"
        variants[1]["cases"] = []
        variants[2]["cases"].append(copy.deepcopy(variants[2]["cases"][0]))
        variants[3]["cases"][0]["result"]["offered_updates"] = 0
        variants[4]["repetitions"] = 2
        for after in variants:
            with self.assertRaises(ValueError):
                module.compare(before, after)

    def test_rejects_invalid_metrics_and_failed_recovery(self):
        for field, value in [("mean_payload_bytes", float("nan")), ("recovered_clients", 0), ("max_final_tick_lag", 99), ("received_updates", 0), ("minimum_client_delivery_ratio", 0)]:
            after = report()
            after["cases"][0]["result"][field] = value
            with self.assertRaises(ValueError):
                module.compare(report(), after)

    def test_delivery_regression_gate_fails_despite_final_recovery(self):
        import json
        import subprocess
        import sys
        import tempfile
        before, after = report(), report()
        after["cases"][0]["result"]["received_updates"] = 60
        after["cases"][0]["result"]["minimum_client_delivery_ratio"] = 0.5
        rows = module.compare(before, after)
        self.assertEqual(rows[0]["metrics"]["delivery_ratio"]["change_percent"], -50)
        with tempfile.TemporaryDirectory() as tmp:
            left, right = Path(tmp) / "before.json", Path(tmp) / "after.json"
            left.write_text(json.dumps(before))
            right.write_text(json.dumps(after))
            result = subprocess.run([sys.executable, module.__file__, str(left), str(right),
                                     "--max-delivery-regression-percent", "0"], capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("delivery regression", result.stderr)

    def test_rejects_missing_condition_even_in_both_reports(self):
        before, after = report(), report()
        before["cases"].pop()
        after["cases"].pop()
        with self.assertRaises(ValueError):
            module.compare(before, after)


if __name__ == "__main__":
    unittest.main()
