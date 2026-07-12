from __future__ import annotations

import argparse
import json
import platform
import subprocess
import wave
from datetime import datetime, timezone
from pathlib import Path

from .lab import CANDIDATE_MODELS, run_json_command, sha256_file


WHISPER_MODELS = {"turbo", "large"}


def build_command(
    executable: Path,
    wav: Path,
    model_id: str,
    ort_accelerator: str | None = None,
    repeat: int = 1,
) -> list[str]:
    command = [
        str(executable),
        "--transcribe-file",
        str(wav),
        "--model",
        model_id,
        "--repeat",
        str(repeat),
        "--json",
    ]
    if model_id in WHISPER_MODELS:
        command.extend(["--device-index", "1"])
    if ort_accelerator is not None:
        command.extend(["--ort-accelerator", ort_accelerator])
    return command


def wav_seconds(path: Path) -> float:
    with wave.open(str(path), "rb") as source:
        return source.getnframes() / source.getframerate()


def main() -> int:
    parser = argparse.ArgumentParser(description="Replay exact saved Handy WAVs through candidate models")
    parser.add_argument("--executable", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--models", nargs="*", default=[spec.model_id for spec in CANDIDATE_MODELS])
    parser.add_argument("--session-limit", type=int)
    parser.add_argument("--session-ids", nargs="*")
    parser.add_argument("--timeout", type=float, default=600)
    parser.add_argument("--ort-accelerator", choices=["auto", "cpu", "directml"])
    parser.add_argument("--repeat", type=int, default=1)
    args = parser.parse_args()

    manifest = json.loads(args.manifest.read_text(encoding="utf-8"))
    sessions = manifest["sessions"]
    if args.session_ids:
        requested_sessions = set(args.session_ids)
        sessions = [row for row in sessions if row["session_id"] in requested_sessions]
    sessions.sort(key=lambda row: wav_seconds(Path(row["wav_path"])))
    if args.session_limit is not None:
        sessions = sessions[: args.session_limit]

    args.output.parent.mkdir(parents=True, exist_ok=True)
    executable_hash = sha256_file(args.executable)
    with args.output.open("a", encoding="utf-8") as target:
        for model_id in args.models:
            for session in sessions:
                wav = Path(session["wav_path"])
                started = datetime.now(timezone.utc).isoformat()
                record: dict[str, object] = {
                    "schema_version": 1,
                    "started_utc": started,
                    "session_id": session["session_id"],
                    "history_id": session["history_id"],
                    "wav_path": str(wav),
                    "wav_sha256": sha256_file(wav),
                    "model": model_id,
                    "executable_sha256": executable_hash,
                    "host": platform.node(),
                    "status": "failed",
                }
                try:
                    completed = run_json_command(
                        build_command(
                            args.executable,
                            wav,
                            model_id,
                            args.ort_accelerator,
                            args.repeat,
                        ),
                        timeout_seconds=args.timeout,
                    )
                    record.update(completed.result)
                    record["process_elapsed_ms"] = completed.elapsed_ms
                    record["stderr"] = completed.stderr
                    record["status"] = "complete"
                except (subprocess.SubprocessError, ValueError) as error:
                    record["error_type"] = type(error).__name__
                    record["error"] = str(error)
                target.write(json.dumps(record, ensure_ascii=False) + "\n")
                target.flush()
                print(
                    json.dumps(
                        {
                            "session_id": record["session_id"],
                            "model": model_id,
                            "status": record["status"],
                            "load_ms": record.get("load_ms"),
                            "best_ms": record.get("best_ms"),
                            "bound_backend": record.get("bound_backend"),
                        }
                    )
                )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
