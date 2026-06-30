# Dictation Workflow, Diagnostics, and Tuning

This branch adds a local dictation workflow for long-form prompts and coding
assistants. The goal is to make Handy behave more like live dictation while
still keeping a final full-utterance transcription pass for accuracy.

All transcription, diagnostics, and audio capture remain local to the machine.

## What Changed

- Progressive dictation can type into the active text field while recording is
  still in progress.
- Dictation stability modes control how aggressively live text is edited.
- Spoken finish phrases can stop dictation without pressing the transcribe
  shortcut again.
- The recording overlay shows compact live diagnostics for edit churn.
- Optional diagnostic capture writes per-session JSONL logs for later review.
- Saved WAV files are linked from diagnostics so accuracy can be checked against
  the original audio.
- A game-mode auto-unload option can free the loaded model when a full-screen game
  is detected.

## Recommended Daily Setup

For ChatGPT-style prompts and coding assistants, use:

- Model: Parakeet V3 or Whisper Turbo, depending on the current accuracy/speed
  tradeoff on the machine.
- Dictation Stability: Stable Live.
- Voice Finish Trigger: enabled.
- Finish phrases: unique phrases such as `finish dictation`, `end dictation`, or
  `send it`.
- Recording retention: at least 3 days while tuning.
- Diagnostic capture: enabled while actively reviewing performance.

## Dictation Stability Modes

The setting is in Settings > General.

### Stable Live

Stable Live is the default recommendation for long-form dictation. It types live
interim text, but it avoids large early rewrites when the new transcript would
delete too much of the existing text. It also locks text at sentence boundaries
once those sentences survive a later pass.

Use this when dictating into:

- ChatGPT or Claude prompts
- Codex or coding assistant prompts
- email and notes
- longer paragraphs where large backspace/retype cycles are distracting

### Fast Live

Fast Live applies interim transcript replacements more aggressively. It can feel
more immediate, but it may rewrite text more often because it does not use the
same sentence-locking behavior.

Use this when:

- very low latency matters more than visual stability
- the target field handles rapid edits well
- the dictated text is short

### Final Only

Final Only disables live progressive text insertion and waits for the final
transcript after recording stops.

Use this when:

- the target application reacts badly to live edits
- you want maximum visual stability
- you are comparing final transcript accuracy without live text behavior

## Spoken Finish Phrases

The setting is in Settings > General.

When Voice Finish Trigger is enabled, Handy checks the tail of live and final
transcripts for configured finish phrases. If a phrase matches, Handy removes
the phrase from the text and stops dictation as though the transcribe shortcut had
been pressed.

Default phrases:

- `finish dictation`
- `end dictation`
- `send it`

The matcher also handles common misrecognitions such as `finish dictate`,
`end dictate`, `finish dictatation`, and `end dictatation`.

The cleanup is tail-only. A phrase in the middle of normal dictated text should
not stop dictation. If the model returns repeated finish phrases at the end, all
contiguous finish phrases are stripped so they do not leak into the pasted text.

## Recording Overlay Diagnostics

The small recording overlay includes a compact diagnostics strip while dictation
is active.

The strip shows:

- current status: `edit`, `skip`, `live`, or `finish`
- latest transcription pass latency
- number of locked sentences
- backspaces and inserted characters for the latest live edit

This is intentionally compact so it is useful during dictation without turning
the overlay into a full debug panel.

## Diagnostic Capture

Diagnostic capture writes one JSONL file per dictation session. Each line is a
single structured event.

Default release-build path:

```text
src-tauri/target/release/Data/diagnostics/
```

Typical filename:

```text
diag-YYYYMMDD-HHMMSS-N.jsonl
```

The linked WAV files are saved by history under:

```text
src-tauri/target/release/Data/recordings/
```

Important events:

- `session_start`: selected model, language, paste method, stability mode, finish
  trigger state
- `recording_start` and `recording_stop`: recording lifecycle and sample counts
- `progressive_audio_chunk`: audio collected for live transcription
- `progressive_interim_transcript`: live transcript text and timing
- `progressive_edit`: live replacement applied to the target field
- `progressive_skip`: live replacement skipped because it was unstable or empty
- `voice_finish_match`: spoken finish phrase detected
- `final_full_transcript`: final full-utterance transcript and timing
- `final_voice_finish_cleanup`: finish phrase stripped from final text
- `final_reconciliation_edit`: final pass replaced live text
- `paste_result`: paste outcome for non-progressive modes
- `history_saved`: saved WAV filename linked to the transcript
- `*_error`: any operation-level diagnostic error

Diagnostic files contain transcript text. Treat them as private user data.

## Reviewing Performance

After using the app for a day or two, run:

```bash
bun run diagnostics:summary
```

This summarizes:

- session count
- model and mode
- number of live transcription passes
- maximum interim latency
- final pass latency
- live backspaces and insertions
- skipped would-be backspaces
- final reconciliation backspaces
- paste status
- linked WAV filename
- error count

The most useful fields for tuning live dictation are:

- `interimMaxMs`: whether live transcription is keeping up
- `liveBackspaces`: how much visible live editing happened
- `skippedBackspaces`: how much unstable editing was prevented
- `finalBackspaces`: how much the final pass still had to rewrite
- `errors`: whether failures are happening silently

High `finalBackspaces` means the final full transcript is materially different
from the live text. That may be acceptable for accuracy, but it is the main signal
to tune stability mode, chunk timing, model choice, or finish phrase behavior.

## Accuracy Test Harness

The `accuracy_test` folder contains a repeatable passage and scoring script.

Workflow:

1. Open a text field.
2. Start dictation.
3. Read `accuracy_test/reference.txt` aloud.
4. Stop dictation.
5. Run:

```bash
python accuracy_test/score.py
```

The script compares the latest history transcript against the reference and
prints WER, word accuracy, CER, and the exact substitutions/deletions/insertions.

To score a specific history row:

```bash
python accuracy_test/score.py --list 10
python accuracy_test/score.py --id 42
```

To score arbitrary text:

```bash
python accuracy_test/score.py --text "transcript to score"
```

## Game-Mode Auto-Unload

Game-mode auto-unload is an Advanced setting. When enabled, Handy can unload
the active model before transcription if it detects a full-screen game process.

Use this when GPU memory pressure matters, especially with large models or other
GPU-heavy applications.

## Practical Tuning Checklist

When reviewing diagnostics:

1. Confirm the model and stability mode in `session_start`.
2. Look at `interimMaxMs` to see whether live transcription is fast enough.
3. Look at `finalBackspaces` to measure how much final text rewrote live text.
4. Open the linked WAV if a transcript looks suspicious.
5. Run `accuracy_test/score.py` for controlled comparisons.
6. Try Stable Live vs Final Only if the target application handles live rewrites
   poorly.
7. Try a different model only after confirming the same WAV performs badly.
