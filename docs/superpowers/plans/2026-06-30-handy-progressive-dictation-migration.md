# Handy Progressive Dictation Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rebase the user's dictation workflow improvements onto current upstream Handy while keeping Handy name, branding, package identity, and dependency direction.

**Architecture:** Start from `cjpais/Handy` `main`, then port only behavior and diagnostics from the WhisperKey-derived `codex/progressive-dictation` branch. Do not carry over WhisperKey brand files, release endpoints, website files, or the vendored `transcribe-rs 0.2.8` dependency tree.

**Tech Stack:** Tauri 2, Rust, React, TypeScript, `transcribe-rs`, `handy-keys`, `cpal`, `vad-rs`, Bun.

---

### Task 1: Baseline and Inventory

**Files:**

- Read: `README.md`
- Read: `src-tauri/Cargo.toml`
- Read: `src-tauri/src/actions.rs`
- Read: `src-tauri/src/settings.rs`
- Read: `src/components/settings/general/GeneralSettings.tsx`
- Read: `src/overlay/RecordingOverlay.tsx`

- [x] **Step 1: Create Handy-based branch**

Run:

```powershell
git fetch --no-tags handy main
git worktree add D:\Codex\Handy-progressive-dictation -b codex/handy-progressive-dictation handy/main
```

Expected: worktree is checked out at current Handy `main` with `productName` and binary still set to Handy.

- [x] **Step 2: Inventory custom WhisperKey changes**

Run:

```powershell
git diff --name-status d59ef5c..c514a16
git diff --stat d59ef5c..c514a16
```

Expected: progressive dictation, voice finish, diagnostics, docs, scripts, and frontend settings are identified; vendored `transcribe-rs-0.2.8` and WhisperKey branding are excluded from the migration scope.

### Task 2: Port Backend Dictation Behavior

**Files:**

- Modify: `src-tauri/src/actions.rs`
- Modify: `src-tauri/src/settings.rs`
- Modify: `src-tauri/src/clipboard.rs`
- Modify: `src-tauri/src/overlay.rs`
- Create: `src-tauri/src/diagnostics.rs`

- [ ] **Step 1: Port settings types**

Add Handy settings for dictation stability mode, voice finish trigger settings, and diagnostic capture, preserving existing serialization compatibility.

- [ ] **Step 2: Port progressive dictation engine**

Move the recording/edit loop from the custom branch into Handy's current `actions.rs`, adapting to Handy's newer `transcribe-rs` API and existing history/transcription managers.

- [ ] **Step 3: Port voice finish detection**

Carry over the repeated-tail trigger stripping logic and regression tests so phrases such as `send it finish dictation` stop recording without leaking trigger text.

- [ ] **Step 4: Port diagnostics writer**

Add per-session JSONL diagnostic logging and optional WAV retention linked by session ID.

- [ ] **Step 5: Run Rust checks**

Run:

```powershell
cd src-tauri
cargo check --lib
cargo test voice_finish_trigger --lib
```

Expected: the library compiles and voice finish trigger regressions pass.

### Task 3: Port Frontend Controls and Overlay

**Files:**

- Modify: `src/components/settings/general/GeneralSettings.tsx`
- Modify: `src/components/settings/advanced/AdvancedSettings.tsx`
- Modify: `src/components/settings/index.ts`
- Create: `src/components/settings/DictationStability.tsx`
- Create: `src/components/settings/VoiceFinishTrigger.tsx`
- Modify: `src/overlay/RecordingOverlay.tsx`
- Modify: `src/overlay/RecordingOverlay.css`
- Modify: `src/stores/settingsStore.ts`
- Modify: `src/i18n/locales/en/translation.json`

- [ ] **Step 1: Add General panel controls**

Place Dictation Stability and Voice Finish Trigger controls in Handy's General settings area, matching the user's requested placement.

- [ ] **Step 2: Add overlay diagnostics**

Show compact edit/latency diagnostics in the recording overlay without replacing Handy's icon or branding.

- [ ] **Step 3: Run frontend checks**

Run:

```powershell
bun run build
```

Expected: TypeScript and Vite build complete without errors.

### Task 4: Documentation and Operator Tools

**Files:**

- Modify: `README.md`
- Create: `docs/dictation-workflow.md`
- Create: `scripts/diagnostics-summary.mjs`
- Create: `accuracy_test/reference.txt`
- Create: `accuracy_test/score.py`
- Modify: `package.json`

- [ ] **Step 1: Rewrite docs for Handy**

Document the workflow using Handy name, Handy paths, and Handy commands.

- [ ] **Step 2: Add diagnostic summary script**

Add `bun run diagnostics:summary` to summarize diagnostic logs from the Handy app data directory or explicit input path.

- [ ] **Step 3: Add repeatable accuracy harness**

Add reference text and WER/CER scoring script for later tuning against saved WAV/transcript pairs.

### Task 5: Build, Runtime Check, Commit, and Push

**Files:**

- Verify: `src-tauri/target/release/handy.exe`
- Verify: `README.md`
- Verify: `docs/dictation-workflow.md`

- [ ] **Step 1: Build release**

Run:

```powershell
bun run tauri build
```

Expected: release binary is produced as Handy, not WhisperKey.

- [ ] **Step 2: Smoke test runtime**

Launch the release binary, confirm the window/tray show Handy, and confirm the shortcut can start/stop recording.

- [ ] **Step 3: Commit and push branch**

Run:

```powershell
git status --short
git add README.md docs scripts accuracy_test package.json src src-tauri
git commit -m "Port progressive dictation to Handy"
git push origin codex/handy-progressive-dictation
```

Expected: branch is pushed and ready for continued testing or PR/repo migration.
