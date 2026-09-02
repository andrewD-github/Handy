# Direct-Prompt Refocus Resume Design

**Date:** 2026-09-02

**Status:** Approved

## Objective

Allow the user's custom direct-prompt mode to pause insertion when focus leaves the dictation target and automatically resume when the original Windows target regains focus. Expose the behavior as an option under Advanced → Output and preserve the customization as a small, testable extension that can be reapplied to later official Handy releases.

## User Experience

Add **Resume direct output when target regains focus** beneath the existing live-text destination setting. Show it only when **Directly in the active prompt** is selected.

When the option is enabled:

1. Direct-prompt text is inserted normally while the original target remains eligible.
2. A physical click or focus change pauses direct insertion. Transcription and the Handy live overlay continue.
3. When the original captured Windows window and focused control are active again, the next streaming snapshot automatically re-arms the target.
4. Text committed while paused is appended once as a catch-up suffix, followed by normal progressive insertion.
5. If recording finishes while another target is active, Handy does not paste the uninserted tail into that target.

The option defaults to disabled for new and upstream-derived configurations, retaining the current fail-closed behavior. It will be enabled explicitly in the user's installed configuration after the candidate passes its release gate.

## Accepted Limitation

Windows target identity does not reliably identify individual HTML inputs inside Chromium/WebView applications. A submitted prompt and a new prompt may share the same top-level window and focused native control. Consequently, automatic refocus resume can treat a newly cleared prompt as the original target and insert a pending suffix.

The user understands and accepts this limitation because automatic return is more useful for their workflow than permanent cancellation. The setting description must state that returning to a reused browser prompt can resume pending text. The existing fail-closed behavior remains available by turning the option off.

## Architecture

### Settings boundary

Add one boolean custom setting whose serialized name clearly identifies direct-prompt behavior. It must:

- use Serde defaulting so official or older settings files load without migration failure;
- default to `false`;
- have one narrow Tauri settings command and generated TypeScript binding;
- be surfaced through the existing settings store and direct-prompt settings component;
- be included in relevant diagnostics without exposing transcript content.

Keep the setting adjacent to `progressive_output_mode` so the custom patch remains easy to locate during an upstream merge.

### Target identity

Separate target identity into:

- a stable native target key: foreground window plus focused control; and
- an interaction epoch: the existing monotonic physical-click marker.

Strict mode continues to require the entire identity, including the epoch, to match. Resume mode uses the stable target key to decide whether the original target has returned, then adopts the latest interaction epoch before attempting a catch-up append.

The input layer remains responsible only for capturing and verifying the current native target and atomically injecting text. It does not own session policy.

### Progressive session state

Keep resume policy inside `progressive_dictation`, not in transcription or action code. A direct-prompt session has three relevant states:

- **Active:** snapshots may append text after target verification.
- **Paused:** snapshots update the session's latest accepted stream text but perform no prompt edit.
- **Stopped:** no future snapshot or finalization may edit a prompt.

With resume disabled, a target mismatch preserves the current transition from Active to Stopped. With resume enabled, a target mismatch moves Active to Paused. Paused returns to Active only when a fresh target capture has the same stable target key as the session's original target. The refreshed interaction epoch becomes the verification baseline for the next append.

The coordinator must calculate catch-up text from the text it actually inserted, not merely the latest snapshot it observed. This prevents missing or duplicating text accumulated while paused.

### Lifecycle and finalization

The action layer passes the selected resume policy when it starts a direct-prompt session. It does not implement focus or diff rules.

If finalization occurs while the original target is active again, the coordinator may re-arm and append only a safe suffix under the existing append-only reconciliation rules. If finalization occurs while paused on another target, the coordinator owns any text already inserted but performs no fallback paste into the active application. History and diagnostics still receive the complete transcription through upstream Handy's existing flow.

Cancellation, a newer generation, an edit failure, monitor failure, or a non-prefix hypothesis remains terminal. The feature never backspaces, replaces text, or redirects output to a different native target.

## Upstream-Merge Strategy

- Retain `progressive_dictation.rs` as the single owner of custom direct-prompt policy.
- Keep Windows capture/injection mechanics in `input.rs` behind narrow target-key and verification operations.
- Keep the UI addition inside the existing `ProgressiveOutputMode` component rather than changing the wider Advanced settings layout.
- Group custom setting fields, commands, diagnostics, and translations beside the existing progressive-output customization and mark their purpose in comments where that improves merge clarity.
- Update the patch inventory and release documentation with every upstream touchpoint.
- Do not fork or modify upstream streaming inference, overlay rendering, history storage, or general paste behavior.

## Testing

Implementation follows test-first development. Required automated cases include:

1. Strict mode still stops permanently after an interaction-epoch or target change.
2. Resume mode pauses rather than edits while another stable target is active.
3. Returning to the original stable target re-arms with the new epoch.
4. The accumulated suffix is inserted exactly once after return.
5. Subsequent snapshots continue progressively without duplication.
6. A different window or control never receives direct-prompt text.
7. Stop/finalize while away does not paste a tail into the active target.
8. Cancellation and stale generations cannot re-arm.
9. The setting defaults off, persists, round-trips, and loads from settings files where it is absent.
10. The Advanced → Output control updates the backend setting and is hidden outside direct-prompt mode.
11. Diagnostics record pause, resume, and skipped-final decisions.

The full Rust, frontend, translation, formatting, build, installer, and real-WAV replay gates remain required. Replay acceptance continues to require append-only output and no stale generation edits. The previous blanket requirement that every physical click permanently invalidates the session is superseded only when the new option is enabled.

## Installation and Rollback

Build and validate the candidate outside `D:\Apps\Handy`. Do not modify `D:\Apps\Handy\Data` during development or replay. Before replacement, stop Handy and create a timestamped backup of the working executable and required runtime assets. Install only after all automated gates pass, enable the new option in the user's settings, relaunch, and verify the installed version and settings load. Restore the backup automatically if installation or launch verification fails.

## Non-goals

- Reliably identifying individual DOM prompt instances inside every browser or WebView.
- Reading or modifying target-application contents to prove the caret position.
- Adding a dedicated re-arm shortcut.
- Resuming into a different native window or focused control.
- Changing model selection, streaming cadence, transcription accuracy, or overlay behavior.
