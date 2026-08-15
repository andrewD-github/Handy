from pathlib import Path

from tools.asr_replay_lab.benchmark import build_command


def test_build_command_pins_gpu_for_whisper() -> None:
    command = build_command(Path("handy.exe"), Path("audio.wav"), "turbo")

    assert command[-2:] == ["--device-index", "1"]


def test_build_command_does_not_claim_device_selection_for_onnx() -> None:
    command = build_command(Path("handy.exe"), Path("audio.wav"), "parakeet-tdt-0.6b-v3")

    assert "--device-index" not in command


def test_build_command_passes_explicit_ort_accelerator() -> None:
    command = build_command(
        Path("handy.exe"), Path("audio.wav"), "parakeet-tdt-0.6b-v3", "directml"
    )

    assert command[-2:] == ["--ort-accelerator", "directml"]


def test_build_command_uses_requested_repeat_count() -> None:
    command = build_command(Path("handy.exe"), Path("audio.wav"), "turbo", repeat=3)

    repeat_index = command.index("--repeat")
    assert command[repeat_index + 1] == "3"


def test_build_command_requests_production_stream_replay() -> None:
    command = build_command(
        Path("handy.exe"),
        Path("audio.wav"),
        "handy-computer/parakeet-unified-en-0.6b-gguf/parakeet-unified-en-0.6b-Q8_0.gguf",
        stream_replay=True,
    )

    assert "--stream-replay" in command
    assert "--realtime" in command
    assert "--ort-accelerator" not in command


def test_build_command_passes_stable_prefix_agreement() -> None:
    command = build_command(
        Path("handy.exe"),
        Path("audio.wav"),
        "streaming-model",
        stream_replay=True,
        stable_prefix_agreement=2,
    )

    index = command.index("--stable-prefix-agreement")
    assert command[index + 1] == "2"
