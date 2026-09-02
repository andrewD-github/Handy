from tools.asr_replay_lab.stream_report import analyze_stream_row


def test_analyze_stream_row_reports_append_only_commits_and_final_tail() -> None:
    row = {
        "stream_updates": [
            {"elapsed_ms": 400, "committed": "hello", "tentative": " wor"},
            {"elapsed_ms": 700, "committed": "hello world", "tentative": ""},
        ],
        "stop_to_final_ms": 90,
        "text": "hello world!",
    }

    metrics = analyze_stream_row(row)

    assert metrics["first_visible_ms"] == 400
    assert metrics["max_update_gap_ms"] == 300
    assert metrics["visible_prefix_violations"] == 0
    assert metrics["final_relation"] == "extension"
    assert metrics["final_tail_chars"] == 1


def test_analyze_stream_row_detects_committed_retraction_and_material_final_conflict() -> None:
    row = {
        "stream_updates": [
            {"elapsed_ms": 100, "committed": "well", "tentative": ""},
            {"elapsed_ms": 200, "committed": "while", "tentative": ""},
        ],
        "stop_to_final_ms": 50,
        "text": "while we wait",
    }

    metrics = analyze_stream_row(row)

    assert metrics["visible_prefix_violations"] == 1
    assert metrics["final_relation"] == "material_conflict"
    assert metrics["final_repair_chars"] > 0
