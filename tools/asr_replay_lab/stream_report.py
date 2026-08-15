from __future__ import annotations

import argparse
import json
import re
import sqlite3
from collections import defaultdict
from pathlib import Path

from .report import normalized_edit_distance, summarize


def edit_distance(left: list[str] | str, right: list[str] | str) -> int:
    previous = list(range(len(right) + 1))
    for left_index, left_item in enumerate(left, start=1):
        current = [left_index]
        for right_index, right_item in enumerate(right, start=1):
            current.append(
                min(
                    current[-1] + 1,
                    previous[right_index] + 1,
                    previous[right_index - 1] + (left_item != right_item),
                )
            )
        previous = current
    return previous[-1]


def analyze_stream_row(row: dict[str, object]) -> dict[str, object]:
    typed = ""
    accepted_times: list[float] = []
    violations = 0
    for update in row.get("stream_updates", []):
        committed = str(update.get("committed", ""))
        if committed == typed:
            continue
        if committed.startswith(typed):
            typed = committed
            accepted_times.append(float(update["elapsed_ms"]))
        else:
            violations += 1

    final = str(row.get("text", ""))
    if final == typed:
        relation = "exact"
    elif final.startswith(typed):
        relation = "extension"
    else:
        relation = "material_conflict"

    gaps = [right - left for left, right in zip(accepted_times, accepted_times[1:])]
    words = lambda text: re.findall(r"\w+|[^\w\s]", text, flags=re.UNICODE)
    return {
        "first_visible_ms": accepted_times[0] if accepted_times else None,
        "max_update_gap_ms": max(gaps) if gaps else None,
        "accepted_updates": len(accepted_times),
        "committed_prefix_violations": violations,
        "typed_text": typed,
        "final_relation": relation,
        "final_tail_chars": len(final) - len(typed) if final.startswith(typed) else None,
        "final_repair_chars": edit_distance(typed, final),
        "final_repair_words": edit_distance(words(typed), words(final)),
        "stop_to_final_ms": float(row.get("stop_to_final_ms", 0)),
    }


def optional_summary(values: list[float]) -> dict[str, float] | None:
    return summarize(values) if values else None


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Summarize production streaming replay without claiming ground-truth accuracy"
    )
    parser.add_argument("--run", type=Path, required=True)
    parser.add_argument("--actual-usage-db", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()

    rows = [
        json.loads(line)
        for line in args.run.read_text(encoding="utf-8").splitlines()
        if line
    ]
    with sqlite3.connect(args.actual_usage_db) as database:
        observed_history = {
            session_id: history or ""
            for session_id, history in database.execute(
                "select session_id, history_transcript from session_texts"
            )
        }

    by_model: dict[str, list[dict[str, object]]] = defaultdict(list)
    for row in rows:
        by_model[str(row["model"])].append(row)

    models = []
    for model, model_rows in sorted(by_model.items()):
        complete = [row for row in model_rows if row.get("status") == "complete"]
        metrics = [analyze_stream_row(row) for row in complete]
        first_visible = [
            float(metric["first_visible_ms"])
            for metric in metrics
            if metric["first_visible_ms"] is not None
        ]
        update_gaps = [
            float(metric["max_update_gap_ms"])
            for metric in metrics
            if metric["max_update_gap_ms"] is not None
        ]
        history_distances = [
            normalized_edit_distance(str(row.get("text", "")), observed_history[str(row["session_id"])])
            for row in complete
            if str(row["session_id"]) in observed_history
        ]
        models.append(
            {
                "model": model,
                "complete_sessions": len(complete),
                "failed_sessions": len(model_rows) - len(complete),
                "first_visible_ms": optional_summary(first_visible),
                "maximum_visible_update_gap_ms": optional_summary(update_gaps),
                "stop_to_final_ms": optional_summary(
                    [float(metric["stop_to_final_ms"]) for metric in metrics]
                ),
                "committed_prefix_violations": sum(
                    int(metric["committed_prefix_violations"]) for metric in metrics
                ),
                "material_final_conflicts": sum(
                    metric["final_relation"] == "material_conflict" for metric in metrics
                ),
                "final_repair_chars": optional_summary(
                    [float(metric["final_repair_chars"]) for metric in metrics]
                ),
                "final_repair_words": optional_summary(
                    [float(metric["final_repair_words"]) for metric in metrics]
                ),
                "normalized_character_distance_to_observed_history": optional_summary(
                    history_distances
                ),
            }
        )

    report = {
        "ground_truth_status": "none_for_actual_usage",
        "accuracy_warning": "History agreement is an observed-output proxy, not WER or CER.",
        "direct_prompt_policy": "Only append a growing committed prefix; tentative text is never typed.",
        "run_rows": len(rows),
        "models": models,
        "failures": [
            {
                "session_id": row.get("session_id"),
                "model": row.get("model"),
                "error_type": row.get("error_type"),
                "error": row.get("error"),
            }
            for row in rows
            if row.get("status") != "complete"
        ],
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"rows": len(rows), "models": len(models), "output": str(args.output)}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
