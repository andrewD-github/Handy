# Upstream Integration Boundary

This branch keeps the progressive-dictation customization as a small patch stack on official Handy.

## Current base

- Stable upstream tag: `v0.9.5`
- Selected post-release fix: `9e534a3` (`fix(portable): keep Hugging Face models in Data`)
- Integration branch: `codex/handy-0.9.5-progressive`
- Rejected experiment preserved outside the branch: stash `rejected-target-content-guard-experiment-before-0.9.5-port`

The installed application under `D:\Apps\Handy` is not modified by development or candidate builds. Installation requires the release checklist and a timestamped executable/runtime backup.

## Custom upstream hooks

The intended custom patch surface is limited to:

- `src-tauri/src/progressive_dictation.rs`: all direct-prompt policy and session state.
- `src-tauri/src/actions.rs`: start, stop, cancel, and fallback lifecycle calls only.
- `src-tauri/src/managers/transcription.rs`: publish upstream committed/tentative snapshots without owning prompt policy.
- `src-tauri/src/input.rs` and `src-tauri/src/clipboard.rs`: guarded platform target capture and a narrow serialized append operation.
- `src-tauri/src/settings.rs`, generated bindings, and one Advanced Settings control: select official overlay or direct-prompt output.
- `src-tauri/src/lib.rs`: construct and inject the progressive manager.
- Focused diagnostics and replay tooling: verify the extension against actual saved usage.

Audio capture, inference engines, model catalogs, stream commitment, stream finalization, history storage, paste transactions, and overlay rendering remain upstream-owned.

## Patch order

1. Pure append-only progressive policy.
2. Guarded prompt editor adapter.
3. Upstream streaming bridge and lifecycle.
4. Setting and migration.
5. Diagnostics and replay gates.
6. Presentation-only theme patch.

Each patch must remain independently testable. Future upgrades start from a new stable upstream tag and reapply this short series; they do not merge an old transcription engine into the new one.

## Rejected design

The clipboard-copy target-content guard is not part of this branch. Its text-only snapshot could destroy rich clipboard formats, and its verify-then-edit sequence retained a same-window race. Any future final-reconciliation guard must have its own failing regression test and preserve upstream's rich clipboard behavior.
