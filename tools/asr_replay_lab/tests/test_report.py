import pytest

from tools.asr_replay_lab.report import percentile, normalized_edit_distance


def test_percentile_uses_linear_interpolation() -> None:
    assert percentile([0, 10, 20, 30], 0.95) == pytest.approx(28.5)


def test_normalized_edit_distance_is_zero_for_equal_text() -> None:
    assert normalized_edit_distance("Hello, world", "Hello, world") == 0


def test_normalized_edit_distance_is_symmetric_and_bounded() -> None:
    forward = normalized_edit_distance("the quick fox", "the slow fox")
    reverse = normalized_edit_distance("the slow fox", "the quick fox")

    assert forward == reverse
    assert 0 < forward < 1
