from __future__ import annotations

import argparse
import csv
import concurrent.futures
import hashlib
import json
import shutil
import subprocess
import tarfile
import tempfile
import urllib.request
import time
from urllib.parse import urlparse
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Iterable


EXACT_JOIN = "exact_wav_history_audio"


class AmbiguousSessionJoin(ValueError):
    """Raised when replay inputs do not have one-to-one observed identifiers."""


class HashMismatch(ValueError):
    """Raised when a downloaded artifact does not match its pinned digest."""


@dataclass(frozen=True)
class ModelSpec:
    model_id: str
    filename: str
    url: str
    sha256: str
    is_archive: bool


@dataclass(frozen=True)
class JsonCommandResult:
    result: dict[str, object]
    stdout: str
    stderr: str
    elapsed_ms: float


CANDIDATE_MODELS = (
    ModelSpec("parakeet-tdt-0.6b-v2", "parakeet-tdt-0.6b-v2-int8", "https://blob.handy.computer/parakeet-v2-int8.tar.gz", "ac9b9429984dd565b25097337a887bb7f0f8ac393573661c651f0e7d31563991", True),
    ModelSpec("parakeet-tdt-0.6b-v3", "parakeet-tdt-0.6b-v3-int8", "https://blob.handy.computer/parakeet-v3-int8.tar.gz", "43d37191602727524a7d8c6da0eef11c4ba24320f5b4730f1a2497befc2efa77", True),
    ModelSpec("turbo", "ggml-large-v3-turbo.bin", "https://blob.handy.computer/ggml-large-v3-turbo.bin", "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69", False),
    ModelSpec("large", "ggml-large-v3-q5_0.bin", "https://blob.handy.computer/ggml-large-v3-q5_0.bin", "d75795ecff3f83b5faa89d1900604ad8c780abd5739fae406de19f23ecd98ad1", False),
    ModelSpec("moonshine-small-streaming-en", "moonshine-small-streaming-en", "https://blob.handy.computer/moonshine-small-streaming-en.tar.gz", "dbb3e1c1832bd88a4ac712f7449a136cc2c9a18c5fe33a12ed1b7cb1cfe9cdd5", True),
    ModelSpec("moonshine-medium-streaming-en", "moonshine-medium-streaming-en", "https://blob.handy.computer/moonshine-medium-streaming-en.tar.gz", "07a66f3bff1c77e75a2f637e5a263928a08baae3c29c4c053fc968a9a9373d13", True),
)


@dataclass(frozen=True)
class SessionInput:
    session_id: str
    history_id: int
    wav_path: Path
    diagnostic_path: Path
    join_status: str

    def to_json(self) -> dict[str, object]:
        value = asdict(self)
        value["wav_path"] = str(self.wav_path)
        value["diagnostic_path"] = str(self.diagnostic_path)
        return value


def validate_session_inputs(rows: Iterable[SessionInput]) -> list[SessionInput]:
    validated = list(rows)
    seen_sessions: set[str] = set()
    seen_history: set[int] = set()
    seen_wavs: set[str] = set()
    for row in validated:
        if row.join_status != EXACT_JOIN:
            raise AmbiguousSessionJoin(f"session {row.session_id} is not exact: {row.join_status}")
        wav_key = str(row.wav_path.resolve()).casefold()
        duplicate = (
            row.session_id in seen_sessions
            or row.history_id in seen_history
            or wav_key in seen_wavs
        )
        if duplicate:
            raise AmbiguousSessionJoin(
                f"session identifiers are not one-to-one: {row.session_id}/{row.history_id}/{row.wav_path}"
            )
        seen_sessions.add(row.session_id)
        seen_history.add(row.history_id)
        seen_wavs.add(wav_key)
    return validated


def sha256_file(path: Path, chunk_size: int = 1024 * 1024) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(chunk_size), b""):
            digest.update(chunk)
    return digest.hexdigest()


def ensure_artifact(spec: ModelSpec, destination: Path) -> Path:
    if destination.is_file() and sha256_file(destination) == spec.sha256:
        return destination
    destination.parent.mkdir(parents=True, exist_ok=True)
    partial = destination.with_name(destination.name + ".partial")
    try:
        request = urllib.request.Request(
            spec.url, headers={"User-Agent": "Handy-ASR-Replay-Lab/1"}
        )
        with urllib.request.urlopen(request) as response, partial.open("wb") as target:
            shutil.copyfileobj(response, target, length=1024 * 1024)
        actual = sha256_file(partial)
        if actual != spec.sha256:
            raise HashMismatch(
                f"{spec.model_id}: expected {spec.sha256}, downloaded {actual}"
            )
        partial.replace(destination)
        return destination
    except Exception:
        partial.unlink(missing_ok=True)
        raise


def install_model(spec: ModelSpec, cache_dir: Path, models_dir: Path) -> Path:
    models_dir.mkdir(parents=True, exist_ok=True)
    artifact_name = Path(urlparse(spec.url).path).name
    artifact = ensure_artifact(spec, cache_dir / artifact_name)
    destination = models_dir / spec.filename
    if destination.exists():
        return destination
    if not spec.is_archive:
        shutil.copy2(artifact, destination)
        return destination

    with tempfile.TemporaryDirectory(prefix=f"{spec.model_id}-", dir=models_dir) as temporary:
        extraction_root = Path(temporary)
        with tarfile.open(artifact, "r:gz") as archive:
            archive.extractall(extraction_root, filter="data")
        nested = extraction_root / spec.filename
        if nested.is_dir():
            shutil.move(str(nested), str(destination))
        else:
            destination.mkdir()
            for child in extraction_root.iterdir():
                shutil.move(str(child), str(destination / child.name))
    return destination


def extract_json_object(output: str) -> dict[str, object]:
    for line in reversed(output.splitlines()):
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(value, dict):
            return value
    raise ValueError("process output did not contain a JSON object")


def run_json_command(
    command: list[str], timeout_seconds: float, env: dict[str, str] | None = None
) -> JsonCommandResult:
    started = time.perf_counter()
    completed = subprocess.run(
        command,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=timeout_seconds,
        check=True,
        env=env,
    )
    elapsed_ms = (time.perf_counter() - started) * 1000
    return JsonCommandResult(
        result=extract_json_object(completed.stdout),
        stdout=completed.stdout,
        stderr=completed.stderr,
        elapsed_ms=elapsed_ms,
    )


def load_sessions(summary_csv: Path, snapshot_root: Path) -> list[SessionInput]:
    rows: list[SessionInput] = []
    with summary_csv.open(newline="", encoding="utf-8-sig") as source:
        for item in csv.DictReader(source):
            if item["status"] != "complete":
                continue
            rows.append(
                SessionInput(
                    session_id=item["session_id"],
                    history_id=int(item["history_id"]),
                    wav_path=snapshot_root / "recordings" / item["wav_file"],
                    diagnostic_path=snapshot_root / "diagnostics" / item["diagnostic_file"],
                    join_status=item["join_status"],
                )
            )
    return validate_session_inputs(rows)


def write_manifest(rows: Iterable[SessionInput], output: Path) -> None:
    validated = validate_session_inputs(rows)
    missing = [str(path) for row in validated for path in (row.wav_path, row.diagnostic_path) if not path.is_file()]
    if missing:
        raise FileNotFoundError("missing replay inputs: " + ", ".join(missing))
    output.parent.mkdir(parents=True, exist_ok=True)
    payload = {
        "schema_version": 1,
        "join_rule": "session_id + history_id + exact WAV filename/timestamps; never directory order",
        "sessions": [row.to_json() for row in validated],
    }
    output.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser(description="Actual-usage ASR replay laboratory")
    parser.add_argument("--summary", type=Path)
    parser.add_argument("--snapshot", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--download-candidates", action="store_true")
    parser.add_argument("--cache", type=Path)
    parser.add_argument("--models", type=Path)
    args = parser.parse_args()
    if args.download_candidates:
        if args.cache is None or args.models is None:
            parser.error("--download-candidates requires --cache and --models")
        with concurrent.futures.ThreadPoolExecutor(max_workers=3) as executor:
            futures = {
                executor.submit(install_model, spec, args.cache, args.models): spec
                for spec in CANDIDATE_MODELS
            }
            for future in concurrent.futures.as_completed(futures):
                spec = futures[future]
                installed = future.result()
                print(json.dumps({"model": spec.model_id, "installed": str(installed)}))
        return 0
    if args.summary is None or args.snapshot is None or args.output is None:
        parser.error("manifest generation requires --summary, --snapshot, and --output")
    rows = load_sessions(args.summary, args.snapshot)
    write_manifest(rows, args.output)
    print(json.dumps({"sessions": len(rows), "manifest": str(args.output)}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
