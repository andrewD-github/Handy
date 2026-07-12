from __future__ import annotations

import argparse
import json
import sqlite3
import statistics
from collections import defaultdict
from pathlib import Path


def percentile(values: list[float], quantile: float) -> float:
    if not values:
        raise ValueError("percentile requires at least one value")
    ordered = sorted(values)
    position = (len(ordered) - 1) * quantile
    lower = int(position)
    upper = min(lower + 1, len(ordered) - 1)
    fraction = position - lower
    return ordered[lower] + (ordered[upper] - ordered[lower]) * fraction


def normalized_edit_distance(left: str, right: str) -> float:
    if left == right:
        return 0.0
    if not left or not right:
        return 1.0
    previous = list(range(len(right) + 1))
    for left_index, left_char in enumerate(left, start=1):
        current = [left_index]
        for right_index, right_char in enumerate(right, start=1):
            current.append(
                min(
                    current[-1] + 1,
                    previous[right_index] + 1,
                    previous[right_index - 1] + (left_char != right_char),
                )
            )
        previous = current
    return previous[-1] / max(len(left), len(right))


def summarize(values: list[float]) -> dict[str, float]:
    return {
        "min": min(values),
        "p50": statistics.median(values),
        "p95": percentile(values, 0.95),
        "max": max(values),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description="Build a text-redacted candidate replay report")
    parser.add_argument("--run", type=Path, required=True)
    parser.add_argument("--actual-usage-db", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()

    rows = [json.loads(line) for line in args.run.read_text(encoding="utf-8").splitlines() if line]
    with sqlite3.connect(args.actual_usage_db) as database:
        observed = {
            row[0]: {"final": row[1] or "", "history": row[2] or ""}
            for row in database.execute(
                "select session_id, final_full_context_transcript, history_transcript from session_texts"
            )
        }

    by_model: dict[str, list[dict[str, object]]] = defaultdict(list)
    complete_by_session: dict[str, dict[str, str]] = defaultdict(dict)
    for row in rows:
        by_model[row["model"]].append(row)
        if row["status"] == "complete":
            complete_by_session[row["session_id"]][row["model"]] = row["text"]

    model_summary: list[dict[str, object]] = []
    for model, model_rows in sorted(by_model.items()):
        complete = [row for row in model_rows if row["status"] == "complete"]
        best_ms = [float(row["best_ms"]) for row in complete]
        rtfs = [float(row["rtf"]) for row in complete]
        history_distances = [
            normalized_edit_distance(str(row["text"]), observed[row["session_id"]]["history"])
            for row in complete
            if row["session_id"] in observed
        ]
        final_distances = [
            normalized_edit_distance(str(row["text"]), observed[row["session_id"]]["final"])
            for row in complete
            if row["session_id"] in observed
        ]
        model_summary.append(
            {
                "model": model,
                "complete_sessions": len(complete),
                "failed_sessions": len(model_rows) - len(complete),
                "inference_ms": summarize(best_ms) if best_ms else None,
                "realtime_factor_audio_seconds_per_inference_second": summarize(rtfs) if rtfs else None,
                "normalized_character_distance_to_observed_history": summarize(history_distances) if history_distances else None,
                "normalized_character_distance_to_observed_diagnostic_final": summarize(final_distances) if final_distances else None,
            }
        )

    pairwise: dict[tuple[str, str], list[float]] = defaultdict(list)
    for session_models in complete_by_session.values():
        models = sorted(session_models)
        for index, left in enumerate(models):
            for right in models[index + 1 :]:
                pairwise[(left, right)].append(
                    normalized_edit_distance(session_models[left], session_models[right])
                )

    report = {
        "ground_truth_status": "none_for_actual_usage",
        "accuracy_warning": "Distances are model/observed-output agreement proxies, not WER or CER.",
        "run_rows": len(rows),
        "models": model_summary,
        "pairwise_model_agreement": [
            {"left": left, "right": right, "sessions": len(values), "normalized_character_distance": summarize(values)}
            for (left, right), values in sorted(pairwise.items())
        ],
        "failures": [
            {"session_id": row["session_id"], "model": row["model"], "error_type": row.get("error_type")}
            for row in rows
            if row["status"] != "complete"
        ],
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"rows": len(rows), "models": len(model_summary), "output": str(args.output)}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
