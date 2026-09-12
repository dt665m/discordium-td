"""Small fixture-wrapper regressions; never starts a build or clock workload."""
import json
from pathlib import Path
import runpy
import tempfile
import unittest

MODULE = runpy.run_path(str(Path(__file__).with_name("qualify-clock.py")))


class ClockReportTests(unittest.TestCase):
    def report(self, mode="realtime", seconds=86400):
        return {"phase": "final", "mode": mode, "requested_seconds": seconds,
                "reference_seconds": seconds, "actual_elapsed_seconds": seconds,
                "ticks": seconds * 60, "completed": True, "error": None,
                "max_debt_ticks": 0, "max_poll_gap_ns": 10_000_000,
                "T004_actual_hour_passed": True, "T007_actual_day_passed": True,
                "drifts": [{"ppm": ppm, "max_samples": 32, "max_target_phase_ticks": 1,
                            "after_warmup_phase": {"samples": 10, "max_abs_ns": 1000}}
                           for ppm in (-1000, 1000)]}

    def result(self, report, **changes):
        inputs = dict(final=report, mode=report["mode"], seconds=report["requested_seconds"],
                      elapsed=86401, stable=True, code=0, interrupted=False,
                      rss={"samples": 3, "first_bytes": 100, "last_bytes": 120}, max_growth=100)
        inputs.update(changes)
        return MODULE["verdict"](**inputs)

    def test_simulated_and_short_elapsed_never_pass_actual_gates(self):
        result = self.result(self.report(mode="simulated"))
        self.assertTrue(result["simulated_regression_passed"])
        self.assertFalse(result["T004_actual_hour_passed"])
        self.assertFalse(result["T007_actual_day_passed"])
        self.assertFalse(self.result(self.report(), elapsed=3599)["T004_actual_hour_passed"])
        result = self.result(self.report(seconds=3600))
        self.assertTrue(result["T004_actual_hour_passed"])
        self.assertFalse(result["T007_actual_day_passed"])

    def test_missing_sign_debt_phase_drift_and_incomplete_fail(self):
        for field, value in [("ticks", 1), ("max_debt_ticks", 1), ("completed", False),
                             ("max_poll_gap_ns", 100_000_001), ("error", "suspended")]:
            report = self.report()
            report[field] = value
            self.assertFalse(self.result(report)["requested_run_completed"], field)
        report = self.report()
        report["drifts"].pop()
        self.assertFalse(self.result(report)["requested_run_completed"])
        report = self.report()
        report["drifts"][0]["after_warmup_phase"]["max_abs_ns"] = 16_666_667
        self.assertFalse(self.result(report)["requested_run_completed"])
        for changes in ({"stable": False}, {"interrupted": True}, {"code": 1}):
            self.assertFalse(self.result(self.report(), **changes)["requested_run_completed"])
        self.assertFalse(self.result(self.report(), rss={})["T007_actual_day_passed"])
        self.assertFalse(self.result(self.report(), max_growth=0)["T007_actual_day_passed"])

    def test_independent_clock_catches_suspend_and_wall_jumps(self):
        monitor = MODULE["Continuity"](100, 1000, maximum_gap=8)
        monitor.observe(105, 1005)
        monitor.observe(110, 1010.1)
        self.assertEqual(monitor.result()["samples"], 2)
        for monotonic, independent in [(105, 1105), (105, 900), (99, 1001),
                                       (200, 1100), (105, 1007)]:
            monitor = MODULE["Continuity"](100, 1000, maximum_gap=8)
            with self.assertRaises(RuntimeError):
                monitor.observe(monotonic, independent)
        # Small successive wall adjustments cannot evade the total-drift check.
        monitor = MODULE["Continuity"](100, 1000, maximum_gap=8)
        monitor.observe(105, 1005.6)
        with self.assertRaises(RuntimeError):
            monitor.observe(110, 1011.2)

    def test_streamed_metrics_require_one_final_and_enforce_bounds(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "metrics.jsonl"
            report = self.report()
            path.write_text(json.dumps(report) + "\n")
            self.assertEqual(MODULE["final_metrics"](path, 2), report)
            for text, limit in [(json.dumps(report) + "\n" + json.dumps(report) + "\n", 2),
                                (json.dumps({"phase": "interval"}) + "\n", 2),
                                (json.dumps(report) + "\n", 0), ("x" * 16385 + "\n", 2)]:
                path.write_text(text)
                with self.assertRaises(ValueError):
                    MODULE["final_metrics"](path, limit)


if __name__ == "__main__":
    unittest.main()
