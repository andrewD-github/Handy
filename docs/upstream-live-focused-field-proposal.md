## Summary

Handy 0.9.x streaming models can show live transcription in Handy's overlay. I have prototyped an optional second destination on Windows: stream the text directly into the text field that was focused when dictation started.

This preserves the normal dictation experience—the words appear at the caret in ChatGPT, Word, Notes, a browser form, etc.—while keeping Handy local and offline. The existing overlay remains the default.

Before preparing a PR, I would like maintainer/community feedback on whether this fits Handy's roadmap and whether a Windows-first implementation is acceptable.

## User experience

An Advanced setting selects the progressive output destination:

- **Handy overlay** (existing behaviour and default)
- **Focused text field** (prototype)

With a native streaming model, text appears progressively at the original caret while the user speaks. The final transcript does not trigger a destructive paragraph replacement.

## Why this is different from final paste

Final paste waits until dictation stops. The proposed mode makes the target application itself the live transcription surface. This is particularly useful for long prompts and documents because the user can see sentence structure and pacing without looking away from the application they are using.

## Safety and stability policy

Streaming ASR hypotheses can change. Typing every revised hypothesis would cause visible backspace storms, so the prototype uses an append-only policy:

1. Display `committed + tentative` text only while it extends the prefix already inserted.
2. Never backspace or retract text during live dictation.
3. At stop, append a compatible final tail only.
4. If the final transcript materially conflicts with inserted text, preserve what the user already saw rather than rewriting a paragraph.

The destination is guarded as well:

- capture the foreground window and focused control at dictation start;
- invalidate the session after a physical mouse click (mouse movement does not invalidate it);
- revalidate the destination immediately before each insertion;
- on Windows, order the revalidation and one atomic Unicode `SendInput` batch against the mouse hook;
- fail closed if destination monitoring becomes unavailable;
- distinguish zero insertion from partial insertion so final fallback cannot duplicate text.

These safeguards address the case where a user clicks Submit while dictation is still finalizing and then starts a new prompt: text from the previous session must not land in the new field.

## Evidence from actual saved usage

I replayed 17 matched, real Handy recordings (16.14 minutes total) through the exact release executable using Nemotron Streaming 3.5 on Vulkan. These were real microphone recordings with the original voice, pacing, pauses and background conditions—not synthetic audio or published model benchmarks.

Results:

| Metric | Result |
|---|---:|
| Completed sessions | 17/17 |
| Failed sessions | 0 |
| First visible text, p50 / p95 | 2.17 s / 2.25 s |
| Stop-to-final, p50 / p95 | 0.16 s / 0.51 s |
| Maximum visible update gap, p50 / p95 | 2.38 s / 3.44 s |
| Visible prefix violations | 0 |
| Visible backspaces/retractions | 0 |
| Material final conflicts | 1/17 |
| Final repair distance, p95 / max | 3.2 / 4 words |

Handy history was treated only as observed prior system output. No trustworthy corrected reference transcript was available, so I am not claiming WER or CER.

The mode is also now being used successfully for normal long-form dictation into ChatGPT on Windows 11.

## Proposed upstream scope

I would keep an upstream PR much smaller than the experimental branch:

- optional `Focused text field` progressive destination;
- append-only progressive coordinator;
- Windows focused-target identity and click invalidation;
- atomic Unicode insertion and partial-insertion handling;
- settings migration, UI, translations and focused tests.

I would exclude local installation scripts, styling changes, private diagnostics, replay data and unrelated portability work.

The design can expose a platform insertion/target-monitor interface, but only Windows has been implemented and validated so far.

## Questions for maintainers

1. Is live transcription directly into the focused field something Handy would consider during the current feature freeze if community interest is demonstrated?
2. Would a Windows-first PR behind an opt-in setting be acceptable, or should macOS/Linux implementations be included in the initial PR?
3. Is append-only live insertion with a non-destructive final-conflict policy the desired UX, or would maintainers prefer a different reconciliation policy?

If there is interest, I can extract the implementation onto a clean branch based on current `main` and submit a focused draft PR with tests.
