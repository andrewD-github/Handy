# Direct-Prompt Refocus Resume Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an optional automatic pause-and-resume policy for custom direct-prompt streaming output when the original native target regains focus.

**Architecture:** `progressive_dictation.rs` remains the sole owner of direct-output policy and tracks accepted versus actually inserted text. `input.rs` exposes only native target capture and verified injection; settings, action lifecycle, diagnostics, and the focused Advanced → Output component receive narrow additions beside the existing custom output-mode integration.

**Tech Stack:** Rust, Tauri 2, Serde, Specta-generated TypeScript bindings, React, Zustand, i18next, Bun/Vite, Windows `SendInput` target verification.

---

### Task 1: Add target-return state-machine tests

**Files:**
- Modify: `src-tauri/src/progressive_dictation.rs`

- [ ] **Step 1: Add failing coordinator tests**

Extend `FakeEditor` with `current_target()` and add tests using the wished-for `RefocusPolicy::Stop` and `RefocusPolicy::ResumeOriginalTarget` API:

```rust
#[test]
fn resume_mode_catches_up_once_when_original_target_returns() {
    let original = TargetIdentity::test_with_interaction(10, 20, 1);
    let other = TargetIdentity::test_with_interaction(10, 30, 2);
    let returned = TargetIdentity::test_with_interaction(10, 20, 3);
    let editor = Arc::new(FakeEditor::new(original));
    let mut coordinator = ProgressiveCoordinator::new(Arc::clone(&editor));
    coordinator.start(7, original, RefocusPolicy::ResumeOriginalTarget);
    assert_eq!(coordinator.apply_snapshot(7, "hello", ""), ProgressiveDecision::Append("hello".into()));
    editor.set_target(other);
    assert_eq!(coordinator.apply_snapshot(7, "hello world", ""), ProgressiveDecision::Paused);
    editor.set_target(returned);
    assert_eq!(coordinator.apply_snapshot(7, "hello world again", ""), ProgressiveDecision::ResumeAppend(" world again".into()));
    assert_eq!(editor.inserts(), vec!["hello", " world again"]);
}
```

Add separate tests proving strict mode remains terminal, a different target receives nothing, duplicate snapshots do not duplicate catch-up text, finalization while away owns existing output without pasting a tail, and cancellation/stale generations cannot re-arm.

- [ ] **Step 2: Run tests and verify RED**

Run: `cargo test --manifest-path src-tauri/Cargo.toml progressive_dictation -- --nocapture`

Expected: compilation fails because `RefocusPolicy`, `Paused`, `ResumeAppend`, and `PromptEditor::current_target` do not exist.

- [ ] **Step 3: Commit the failing tests**

Run:

```powershell
git add src-tauri/src/progressive_dictation.rs
git commit -m "test: define direct-prompt refocus resume"
```

### Task 2: Implement pause, return detection, and catch-up

**Files:**
- Modify: `src-tauri/src/progressive_dictation.rs`
- Modify: `src-tauri/src/input.rs`

- [ ] **Step 1: Add stable target-key operations**

Add methods that keep the interaction epoch separate from the stable native target:

```rust
impl TargetIdentity {
    fn same_native_target(self, current: Self) -> bool {
        self.foreground_window == current.foreground_window
            && self.focused_control == current.focused_control
    }

    fn with_interaction_epoch(self, interaction_epoch: u64) -> Self {
        Self { interaction_epoch, ..self }
    }
}
```

Retain exact `matches` semantics for atomic verified insertion.

- [ ] **Step 2: Add explicit policy and editor capture API**

Add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RefocusPolicy { Stop, ResumeOriginalTarget }

pub(crate) trait PromptEditor {
    fn current_target(&self) -> Option<TargetIdentity>;
    fn append_verified(&self, expected_target: TargetIdentity, text: &str)
        -> Result<(), PromptEditError>;
}
```

`RuntimePromptEditor::current_target` delegates to `crate::input::capture_target_identity`; the fake editor returns its controlled target.

- [ ] **Step 3: Track accepted and inserted output separately**

Keep `ProgressiveSession::displayed` as accepted append-only stream text. Add coordinator fields `inserted: String`, `refocus_policy`, and `paused`. On every accepted or duplicate snapshot:

1. Capture the current target.
2. In strict mode, cancel on any exact mismatch.
3. In resume mode, return `Paused` while the native target differs.
4. When the native target matches again, refresh the session target to the current interaction epoch.
5. Compute pending text with `session.displayed().strip_prefix(&self.inserted)` and inject it once.
6. Return `ResumeAppend(pending)` when transitioning from paused, otherwise `Append(pending)`.

Verified-injection races remain fail-closed: `TargetChanged` pauses in resume mode and stops in strict mode; partial or other edit failures remain terminal.

- [ ] **Step 4: Apply the same target gate to finalization**

Finalization may append a safe final suffix only when the original native target is active. While away, return `TargetChanged` with `owns_output: true`; never invoke upstream fallback paste into the active target.

- [ ] **Step 5: Run tests and verify GREEN**

Run: `cargo test --manifest-path src-tauri/Cargo.toml progressive_dictation -- --nocapture`

Expected: all progressive-dictation tests pass, including the new pause/resume cases.

- [ ] **Step 6: Commit runtime behavior**

Run:

```powershell
git add src-tauri/src/progressive_dictation.rs src-tauri/src/input.rs
git commit -m "feat: resume direct output on target return"
```

### Task 3: Add persisted setting and lifecycle wiring

**Files:**
- Modify: `src-tauri/src/settings.rs`
- Modify: `src-tauri/src/shortcut/mod.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/src/actions.rs`
- Modify: `src-tauri/src/diagnostics.rs`
- Modify/generated: `src/bindings.ts`
- Modify: `src/stores/settingsStore.ts`

- [ ] **Step 1: Write failing settings tests**

Add tests asserting `direct_prompt_resume_on_refocus` defaults to `false`, survives JSON round-trip when `true`, and absent legacy JSON loads as `false`.

```rust
#[test]
fn direct_prompt_refocus_resume_defaults_off_and_round_trips() {
    let defaults = get_default_settings();
    assert!(!defaults.direct_prompt_resume_on_refocus);
    let mut enabled = defaults;
    enabled.direct_prompt_resume_on_refocus = true;
    let restored: AppSettings = serde_json::from_value(serde_json::to_value(enabled).unwrap()).unwrap();
    assert!(restored.direct_prompt_resume_on_refocus);
}
```

- [ ] **Step 2: Run settings test and verify RED**

Run: `cargo test --manifest-path src-tauri/Cargo.toml direct_prompt_refocus -- --nocapture`

Expected: compilation fails because the field is missing.

- [ ] **Step 3: Add the setting, command, diagnostics, and action policy**

Add to `AppSettings` beside `progressive_output_mode`:

```rust
#[serde(default)]
pub direct_prompt_resume_on_refocus: bool,
```

Default it to `false`. Add `change_direct_prompt_resume_on_refocus_setting(app, enabled)`, register it in `collect_commands!`, record it in diagnostic `session_start`, and start the manager with:

```rust
progressive_dictation.start(settings.direct_prompt_resume_on_refocus)
```

Map the boolean to `RefocusPolicy` inside the manager.

- [ ] **Step 4: Regenerate bindings and connect Zustand**

Run the debug application once in headless list mode, which executes the Specta export at `src-tauri/src/lib.rs:852` before entering the headless path:

```powershell
cargo run --manifest-path src-tauri/Cargo.toml -- --list-models
```

Then verify `src/bindings.ts` contains the command and settings field. Add:

```ts
direct_prompt_resume_on_refocus: (value) =>
  commands.changeDirectPromptResumeOnRefocusSetting(value as boolean),
```

to `settingUpdaters`.

- [ ] **Step 5: Run focused and full backend tests**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml direct_prompt_refocus -- --nocapture
cargo test --manifest-path src-tauri/Cargo.toml progressive_dictation -- --nocapture
```

Expected: both commands exit successfully with zero failures.

- [ ] **Step 6: Commit settings integration**

Run:

```powershell
git add src-tauri/src/settings.rs src-tauri/src/shortcut/mod.rs src-tauri/src/lib.rs src-tauri/src/actions.rs src-tauri/src/diagnostics.rs src/bindings.ts src/stores/settingsStore.ts
git commit -m "feat: configure direct-output refocus resume"
```

### Task 4: Add Advanced → Output control and integration documentation

**Files:**
- Modify: `src/components/settings/ProgressiveOutputMode.tsx`
- Modify: `src/i18n/locales/*/translation.json`
- Modify: `docs/upstream-integration.md`
- Modify: `docs/progressive-release-checklist.md`

- [ ] **Step 1: Add the conditional toggle**

Render a `ToggleSwitch` after the destination dropdown only when `selected === "direct_prompt"`:

```tsx
{selected === "direct_prompt" && (
  <ToggleSwitch
    checked={getSetting("direct_prompt_resume_on_refocus") ?? false}
    onChange={(enabled) =>
      updateSetting("direct_prompt_resume_on_refocus", enabled)
    }
    isUpdating={isUpdating("direct_prompt_resume_on_refocus")}
    label={t("settings.advanced.progressiveOutput.resumeOnRefocus.label")}
    description={t("settings.advanced.progressiveOutput.resumeOnRefocus.description")}
    descriptionMode={descriptionMode}
    grouped={grouped}
  />
)}
```

The English description must disclose that reused browser prompts may receive pending text. Mirror the source text into every locale, matching the repository's current policy for untranslated custom strings.

- [ ] **Step 2: Update the upstream patch inventory and release checklist**

Document the new setting/command/UI touchpoints, the `ResumeOriginalTarget` policy boundary, the accepted WebView limitation, and live validation steps for click-away, return, Submit, and stop-while-away.

- [ ] **Step 3: Run frontend and translation checks**

Run:

```powershell
bun run check:translations
bun run lint
bun run build
bun run format:check
```

Expected: every command exits successfully with no missing translation keys, lint errors, type errors, build failures, or formatting differences.

- [ ] **Step 4: Commit UI and documentation**

Run:

```powershell
git add src/components/settings/ProgressiveOutputMode.tsx src/i18n/locales docs/upstream-integration.md docs/progressive-release-checklist.md
git commit -m "feat: expose direct-output refocus resume"
```

### Task 5: Release gate, installation, and observation

**Files:**
- Modify: `docs/releases/2026-09-02-handy-0.9.6-refocus-resume.md`

- [ ] **Step 1: Run the complete automated gate**

Run:

```powershell
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
bun run check:translations
bun run lint
bun run build
bun run format:check
python -m pytest tools/asr_replay_lab/tests -q
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/test-install-portable-progressive.ps1
```

Build the replay executable and run the same 17 real-WAV sessions using the exact manifest and benchmark command recorded by `D:\Apps\Handy\Data\analysis\candidate-0.9.6-progressive`; write this run to `D:\Apps\Handy\Data\analysis\candidate-0.9.6-refocus-resume` so the verified baseline remains immutable.

Expected: zero test failures, zero backspaces/retractions, no stale-generation edits, no insertion into a different native target, and no material latency regression relative to the recorded v0.9.6 baseline.

- [ ] **Step 2: Build and inspect the candidate**

Build the Windows release executable/package outside `D:\Apps\Handy`, verify version `0.9.6`, calculate SHA-256, and record the artifact paths and results in the new release report.

- [ ] **Step 3: Back up and install transactionally**

Stop Handy, use the existing installer to create a timestamped backup, replace only the executable/runtime assets, preserve `D:\Apps\Handy\Data`, and set `direct_prompt_resume_on_refocus` to `true` without changing the selected model or other user settings.

- [ ] **Step 4: Launch and verify the installed surface**

Verify the installed process launches, reports `0.9.6`, loads the existing model/settings/history, shows the new Advanced → Output option enabled, and records no startup migration error. Exercise the click-away/return flow in a safe disposable text field if desktop automation can do so without interfering with the user's active work; otherwise report that specific live interaction as not observed.

- [ ] **Step 5: Commit the release evidence**

Run:

```powershell
git add docs/releases
git commit -m "docs: record refocus-resume release gate"
```
