# Upstream Integration Boundary

This branch keeps the progressive-dictation customization as a small patch stack on official Handy.

## Current base

- Stable upstream tag: `v0.9.6` (`af48dd6`)
- The earlier portable Hugging Face data fix is included upstream in this release.
- Integration branch: `codex/handy-progressive-dictation`
- Rejected experiment preserved outside the branch: stash `rejected-target-content-guard-experiment-before-0.9.5-port`

The installed application under `D:\Apps\Handy` is not modified by development or candidate builds. Installation requires the release checklist and a timestamped executable/runtime backup.

## Custom upstream hooks

The intended custom patch surface is limited to:

- `src-tauri/src/progressive_dictation.rs`: all direct-prompt policy and session state.
- `src-tauri/src/actions.rs`: start, stop, cancel, and fallback lifecycle calls only.
- `src-tauri/src/managers/transcription.rs`: publish upstream committed/tentative snapshots without owning prompt policy.
- `src-tauri/src/input.rs` and `src-tauri/src/clipboard.rs`: guarded platform target capture, click-without-mouse-move interaction epochs, and a narrow serialized append operation.
- `src-tauri/src/settings.rs`, `src-tauri/src/shortcut/mod.rs`, generated bindings, the settings store, and one Advanced Settings control: select official overlay or direct-prompt output and configure optional refocus resume.
- `src-tauri/src/lib.rs`: construct and inject the progressive manager.
- Focused diagnostics and replay tooling: verify the extension against actual saved usage. The `replay-console` Cargo feature only retains stdout/stderr for optimized headless measurements; normal packaged builds keep the Windows GUI subsystem.
- `src/styles/theme.css`: the custom blue presentation is expressed through upstream theme tokens. Components consume tokens rather than owning a second palette.

Audio capture, inference engines, model catalogs, stream commitment, stream finalization, history storage, paste transactions, and overlay rendering remain upstream-owned.

## Patch order

1. Pure append-only progressive policy.
2. Guarded prompt editor adapter.
3. Upstream streaming bridge and lifecycle.
4. Setting and migration.
5. Diagnostics and replay gates.
6. Submit/click residual-text guard and optional original-target refocus policy.
7. Presentation-only token theme patch.

Each patch must remain independently testable. Future upgrades merge the complete new stable upstream tag into this branch, resolve the documented hook points, and replay the frozen real-use cohort; they do not copy an old transcription engine into the new one.

## Rejected design

The clipboard-copy target-content guard is not part of this branch. Its text-only snapshot could destroy rich clipboard formats, and its verify-then-edit sequence retained a same-window race. Any future final-reconciliation guard must have its own failing regression test and preserve upstream's rich clipboard behavior.

The accepted residual-text guard does not inspect or replace clipboard content. A physical pointer button press increments an interaction epoch. Pointer movement is intentionally ignored. With `direct_prompt_resume_on_refocus` disabled, a direct-prompt session only edits while its captured epoch still matches and fails closed after a submit click even when Chromium reuses the same window and focused control.

With `direct_prompt_resume_on_refocus` enabled, `progressive_dictation.rs` separates the stable native target (foreground window plus focused control) from the interaction epoch. It pauses while another native target is active, adopts the new epoch when the original native target returns, and inserts the accumulated append-only suffix once. This policy deliberately accepts that Chromium/WebView may reuse the same native control for a newly cleared prompt after Submit. The setting defaults off so upstream-derived configurations retain the stricter guard.
