from __future__ import annotations

import argparse
import json
import sqlite3
from pathlib import Path


TARGETS = {
    "First visible text": 1200.0,
    "Visible update gap": 750.0,
    "Stop to final": 1500.0,
}

BASELINE_KEYS = {
    "First visible text": "p95_first_visible_ms",
    "Visible update gap": "p95_update_gap_ms",
    "Stop to final": "p95_stop_to_final_ms",
}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--validation", type=Path, required=True)
    parser.add_argument("--candidate-summary", type=Path, required=True)
    parser.add_argument("--directml-summary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()

    validation = json.loads(args.validation.read_text(encoding="utf-8"))
    candidate = json.loads(args.candidate_summary.read_text(encoding="utf-8"))
    directml = json.loads(args.directml_summary.read_text(encoding="utf-8"))
    args.output.parent.mkdir(parents=True, exist_ok=True)

    with sqlite3.connect(args.output) as database:
        database.executescript(
            """
            drop table if exists baseline_targets;
            create table baseline_targets(metric text primary key, observed_p95_ms real not null, target_ms real not null, sessions integer not null);
            drop table if exists candidate_latency;
            create table candidate_latency(configuration text primary key, engine text not null, provider text not null, p50_ms real not null, p95_ms real not null, max_ms real not null, complete_sessions integer not null, failed_sessions integer not null, agreement_p95 real);
            """
        )
        database.executemany(
            "insert into baseline_targets values (?, ?, ?, ?)",
            [
                (metric, validation[key], TARGETS[metric], validation["complete_sessions"])
                for metric, key in BASELINE_KEYS.items()
            ],
        )

        rows: list[tuple[object, ...]] = []
        for report, provider_suffix in ((candidate, None), (directml, "DirectML")):
            for model in report["models"]:
                name = model["model"]
                if provider_suffix and not name.startswith("parakeet"):
                    continue
                provider = provider_suffix or ("RTX 3080 Vulkan" if name in {"turbo", "large"} else "CPU via Auto")
                configuration = f"{name} / {provider_suffix or provider}"
                distance = model["normalized_character_distance_to_observed_history"]
                rows.append(
                    (
                        configuration,
                        name,
                        provider,
                        model["inference_ms"]["p50"],
                        model["inference_ms"]["p95"],
                        model["inference_ms"]["max"],
                        model["complete_sessions"],
                        model["failed_sessions"],
                        distance["p95"] if distance else None,
                    )
                )
        database.executemany("insert into candidate_latency values (?, ?, ?, ?, ?, ?, ?, ?, ?)", rows)
        database.commit()
        integrity = database.execute("pragma integrity_check").fetchone()[0]
        if integrity != "ok":
            raise RuntimeError(f"decision database integrity check failed: {integrity}")
    print(json.dumps({"baseline_rows": len(BASELINE_KEYS), "candidate_rows": len(rows), "database": str(args.output)}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
