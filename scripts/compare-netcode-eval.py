#!/usr/bin/env python3
"""Compare matching eval workloads; timing uses medians across repetitions."""
import argparse
import json
import math
import statistics

METRICS = {
    "bytes_per_update": ("mean_payload_bytes",),
    "server_kib_per_second": ("server_renet_kib_per_second",),
    "packets_per_tick": ("server_renet_packets_per_tick",),
    "replication_p95_us": ("replication_us", "p95"),
    "transport_p95_us": ("transport_us", "p95"),
    "simulation_p95_us": ("simulation_us", "p95"),
    "history_bytes": ("peak_accounted_history_bytes",),
    "delivery_ratio": ("delivery_ratio",),
    "minimum_client_delivery_ratio": ("minimum_client_delivery_ratio",),
    "snapshot_age_p95_ticks": ("measured_snapshot_age_ticks", "p95"),
    "snapshot_age_max_ticks": ("measured_snapshot_age_ticks", "max"),
}


def indexed(report):
    if report.get("schema_version") != 2 or report.get("profile") != "release":
        raise ValueError("comparison requires schema 2 release reports")
    cases = {}
    for row in report["cases"]:
        key = (row["players"], row["entities"], row["condition"])
        repetition = row["repetition"]
        group = cases.setdefault(key, {})
        if repetition in group:
            raise ValueError(f"duplicate case/repetition: {key}, {repetition}")
        result = row["result"]
        if result["measured_ticks"] <= 0 or result["offered_updates"] <= 0:
            raise ValueError(f"empty measurement: {key}")
        if result["received_updates"] <= 0 or result["minimum_client_delivery_ratio"] <= 0:
            raise ValueError(f"no measured delivery: {key}")
        if result["recovered_clients"] != row["players"] or result["max_final_tick_lag"] > 2:
            raise ValueError(f"failed recovery: {key}")
        group[repetition] = row
    expected = {(players, entities, condition) for players in report["players"]
                for entities in report["entities"] for condition in ["Clean", "MixedLoss"]}
    if not cases or cases.keys() != expected:
        raise ValueError("incomplete workload matrix")
    if any(set(rows) != set(range(report["repetitions"])) for rows in cases.values()):
        raise ValueError("incomplete repetition coverage")
    return cases


def value(row, path):
    result = row["result"]
    if path == ("delivery_ratio",):
        result = result["received_updates"] / result["offered_updates"]
        path = ()
    for field in path:
        result = result[field]
    if not isinstance(result, (int, float)) or not math.isfinite(result) or result < 0:
        raise ValueError(f"invalid metric {path}: {result}")
    return result


def compare(before, after):
    for field in ["target", "ticks", "repetitions", "players", "entities", "seed", "warmup_ticks"]:
        if before[field] != after[field]:
            raise ValueError(f"mismatched {field}")
    left, right = indexed(before), indexed(after)
    if left.keys() != right.keys():
        raise ValueError("case coverage differs")
    comparison = []
    for key in sorted(left):
        for repetition in left[key]:
            a, b = left[key][repetition], right[key][repetition]
            if a["workload_signature"] != b["workload_signature"]:
                raise ValueError(f"workload changed: {key}")
            for field in ["measured_ticks", "offered_updates"]:
                if a["result"][field] != b["result"][field]:
                    raise ValueError(f"measurement coverage changed: {key}, {field}")
        metrics = {}
        for name, path in METRICS.items():
            old = statistics.median(value(row, path) for row in left[key].values())
            new = statistics.median(value(row, path) for row in right[key].values())
            metrics[name] = {"before": old, "after": new,
                             "change_percent": (new / old - 1) * 100 if old else (0 if not new else None)}
        comparison.append({"players": key[0], "entities": key[1], "condition": key[2], "metrics": metrics})
    return comparison


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("before")
    parser.add_argument("after")
    parser.add_argument("--output")
    parser.add_argument("--max-bandwidth-regression-percent", type=float)
    parser.add_argument("--max-cpu-regression-percent", type=float)
    parser.add_argument("--max-delivery-regression-percent", type=float)
    parser.add_argument("--max-snapshot-age-regression-percent", type=float)
    args = parser.parse_args()
    with open(args.before) as source:
        before = json.load(source)
    with open(args.after) as source:
        after = json.load(source)
    rows = compare(before, after)
    text = json.dumps(rows, indent=2) + "\n"
    if args.output:
        with open(args.output, "w") as dest:
            dest.write(text)
    else:
        print(text, end="")
    for row in rows:
        for metric, limit in [("server_kib_per_second", args.max_bandwidth_regression_percent),
                              ("replication_p95_us", args.max_cpu_regression_percent)]:
            change = row["metrics"][metric]["change_percent"]
            if limit is not None and (change is None or change > limit):
                raise SystemExit(f"regression threshold exceeded: {row['players']} players, {row['entities']} entities, {row['condition']}: {metric}")
        for metric in ["delivery_ratio", "minimum_client_delivery_ratio"]:
            change = row["metrics"][metric]["change_percent"]
            if args.max_delivery_regression_percent is not None and (change is None or change < -args.max_delivery_regression_percent):
                raise SystemExit(f"delivery regression threshold exceeded: {metric}")
        change = row["metrics"]["snapshot_age_p95_ticks"]["change_percent"]
        if args.max_snapshot_age_regression_percent is not None and (change is None or change > args.max_snapshot_age_regression_percent):
            raise SystemExit("snapshot age regression threshold exceeded")


if __name__ == "__main__":
    main()
