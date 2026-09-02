import hashlib
import io
import json
import subprocess
import sys
import tarfile
import urllib.request
from pathlib import Path

import pytest

from tools.asr_replay_lab.lab import (
    AmbiguousSessionJoin,
    HashMismatch,
    ModelSpec,
    ensure_artifact,
    install_model,
    run_json_command,
    SessionInput,
    extract_json_object,
    sha256_file,
    validate_session_inputs,
)


def session(**overrides: object) -> SessionInput:
    values: dict[str, object] = {
        "session_id": "diag-1",
        "history_id": 11,
        "wav_path": Path("one.wav"),
        "diagnostic_path": Path("diag-1.jsonl"),
        "join_status": "exact_wav_history_audio",
    }
    values.update(overrides)
    return SessionInput(**values)


def test_validate_session_inputs_accepts_unique_exact_joins() -> None:
    rows = [session(), session(session_id="diag-2", history_id=12, wav_path=Path("two.wav"))]

    assert validate_session_inputs(rows) == rows


@pytest.mark.parametrize(
    "second",
    [
        session(session_id="diag-1", history_id=12, wav_path=Path("two.wav")),
        session(session_id="diag-2", history_id=11, wav_path=Path("two.wav")),
        session(session_id="diag-2", history_id=12, wav_path=Path("one.wav")),
    ],
)
def test_validate_session_inputs_rejects_ambiguous_identifiers(second: SessionInput) -> None:
    with pytest.raises(AmbiguousSessionJoin):
        validate_session_inputs([session(), second])


def test_validate_session_inputs_rejects_non_exact_join() -> None:
    with pytest.raises(AmbiguousSessionJoin, match="not exact"):
        validate_session_inputs([session(join_status="timestamp_only")])


def test_sha256_file_streams_file(tmp_path: Path) -> None:
    payload = b"actual saved audio bytes" * 1000
    artifact = tmp_path / "artifact.bin"
    artifact.write_bytes(payload)

    assert sha256_file(artifact) == hashlib.sha256(payload).hexdigest()


def test_extract_json_object_uses_last_valid_json_line() -> None:
    output = 'native log line\n{"warmup": true}\nmore logging\n{"transcript":"final","best_ms":12}\n'

    assert extract_json_object(output) == {"transcript": "final", "best_ms": 12}


def test_extract_json_object_rejects_output_without_json() -> None:
    with pytest.raises(ValueError, match="JSON object"):
        extract_json_object("logs only")


def test_run_json_command_captures_result_and_streams(tmp_path: Path) -> None:
    completed = run_json_command(
        [sys.executable, "-c", 'import sys; print("log"); print("warning", file=sys.stderr); print(r\'{"best_ms": 7}\')'],
        timeout_seconds=5,
    )

    assert completed.result == {"best_ms": 7}
    assert "log" in completed.stdout
    assert "warning" in completed.stderr
    assert completed.elapsed_ms > 0


def test_run_json_command_enforces_timeout() -> None:
    with pytest.raises(subprocess.TimeoutExpired):
        run_json_command([sys.executable, "-c", "import time; time.sleep(2)"], timeout_seconds=0.05)


def test_ensure_artifact_downloads_and_verifies_file_url(tmp_path: Path) -> None:
    source = tmp_path / "source.bin"
    source.write_bytes(b"model bytes")
    destination = tmp_path / "cache" / "model.bin"
    spec = ModelSpec(
        model_id="model",
        filename="model.bin",
        url=source.as_uri(),
        sha256=sha256_file(source),
        is_archive=False,
    )

    assert ensure_artifact(spec, destination) == destination
    assert destination.read_bytes() == b"model bytes"


def test_ensure_artifact_rejects_wrong_hash(tmp_path: Path) -> None:
    source = tmp_path / "source.bin"
    source.write_bytes(b"corrupt")
    spec = ModelSpec("model", "model.bin", source.as_uri(), "0" * 64, False)

    with pytest.raises(HashMismatch):
        ensure_artifact(spec, tmp_path / "model.bin")


def test_ensure_artifact_identifies_replay_client(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    payload = b"model"

    def open_request(request: urllib.request.Request) -> io.BytesIO:
        assert request.get_header("User-agent") == "Handy-ASR-Replay-Lab/1"
        return io.BytesIO(payload)

    monkeypatch.setattr(urllib.request, "urlopen", open_request)
    spec = ModelSpec("model", "model.bin", "https://models.invalid/model.bin", hashlib.sha256(payload).hexdigest(), False)

    ensure_artifact(spec, tmp_path / "model.bin")


def test_install_model_extracts_directory_archive(tmp_path: Path) -> None:
    payload = tmp_path / "payload"
    model_dir = payload / "engine-files"
    model_dir.mkdir(parents=True)
    (model_dir / "model.onnx").write_bytes(b"onnx")
    archive = tmp_path / "engine.tar.gz"
    with tarfile.open(archive, "w:gz") as target:
        target.add(model_dir, arcname="engine-files")
    spec = ModelSpec("engine", "engine-files", archive.as_uri(), sha256_file(archive), True)

    installed = install_model(spec, tmp_path / "cache", tmp_path / "models")

    assert installed == tmp_path / "models" / "engine-files"
    assert (installed / "model.onnx").read_bytes() == b"onnx"
