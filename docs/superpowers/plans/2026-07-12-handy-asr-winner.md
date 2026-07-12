# Handy Evidence-Backed ASR Winner Implementation Plan

> Execute this plan in the isolated `codex/handy-asr-winner` worktree. All replay inputs are saved real-use WAVs; all large models, builds, and generated evidence stay on `D:`.

**Goal:** Select and implement the fastest accurate architecture supported by the user's actual Handy sessions, with bounded/stale-safe scheduling and atomic user-visible corrections.

**Evidence rule:** Handy outputs are observations, not references. Report WER/CER only for sessions with trustworthy human-corrected references. Otherwise label text comparisons as agreement, churn, or adjudication candidates.

**Safety rule:** Never benchmark against or overwrite `D:\Apps\Handy\handy.exe` while the installed instance is running. Preserve the installed executable, settings, history, recordings, diagnostics, and logs.

---

## Task 1: Freeze inputs and define the replay contract

- [ ] Record hashes and metadata for the 17 exactly joined WAV/history/diagnostic sessions.
- [ ] Define a machine-readable replay manifest keyed by session ID, history ID, WAV filename, and timestamps.
- [ ] Add tests rejecting positional/directory-order joins and ambiguous matches.
- [ ] Generate the manifest from the frozen snapshot and validate all relationships.

## Task 2: Build the isolated real-WAV replay lab

- [ ] Create failing tests for model manifests, resumable downloads, SHA-256 checks, subprocess JSON capture, timeouts, and artifact provenance.
- [ ] Implement the smallest Python replay controller under `tools/asr-replay-lab/`.
- [ ] Create `D:\Handy-ASR-Lab` with separate `bin`, `models`, `runs`, `cache`, and `reports` directories.
- [ ] Build a dedicated release headless executable using `CARGO_TARGET_DIR=D:\h-target`; do not invoke the installed application.
- [ ] Smoke-test one short copied WAV and retain stdout, stderr, executable hash, model hash, and environment facts.

## Task 3: Prove runtime providers

- [ ] Add headless JSON fields that distinguish requested accelerator, bound backend/provider, physical adapter, and fallback.
- [ ] Add unit tests for provider reporting and explicit fallback states.
- [ ] Capture Whisper/Vulkan device enumeration and NVIDIA process telemetry.
- [ ] Capture ONNX Runtime provider evidence for Parakeet/Moonshine; if the library cannot prove it, label provider unverified rather than inferring GPU use.

## Task 4: Benchmark candidate engines on identical real speech

- [ ] Download and hash Parakeet V2, current Parakeet V3, Whisper large-v3-turbo, Whisper large-v3-q5_0, Moonshine small streaming, and Moonshine medium streaming.
- [ ] Replay all 17 complete sessions through every buildable candidate, with deterministic repeats and cold/warm load separation.
- [ ] Measure load time, per-pass inference, real-time factor, GPU utilization, VRAM, CPU, final latency, determinism, transcript agreement, omissions, hallucination flags, punctuation, and capitalization proxies.
- [ ] Retain failures and unsupported configurations in the matrix instead of silently dropping them.

## Task 5: Benchmark competing scheduler architectures

- [ ] Reproduce the serialized cumulative-audio baseline from each session's observed prefix endpoints.
- [ ] Test adaptive cumulative scheduling, latest-only bounded scheduling, genuine stateful streaming plus finalizer, and separate parallel primary/finalizer sessions.
- [ ] Tag every job/result with session, generation, and audio endpoint; discard stale results in the replay simulator.
- [ ] Measure p50/p95 first-visible time, update gap, audio-to-text lag, stop-to-final, total inference, churn, maximum rewrite, queue delay, stale work, and GPU contention.
- [ ] Demonstrate measured lower bounds for any acceptance target no candidate reaches.

## Task 6: Create accuracy evidence without manufacturing ground truth

- [ ] Search the frozen data for independently corrected transcripts and record provenance.
- [ ] Build a compact blinded adjudication artifact from materially different model outputs for sessions lacking references.
- [ ] Compute WER/CER only for verified references; otherwise report pairwise agreement and qualitative error categories.
- [ ] Select the winner using final-reference quality where available, then latency/stability/safety; state uncertainty explicitly.

## Task 7: Implement the evidence-backed winner test-first

- [x] Add failing scheduler tests for accepted-result coalescing, generation ordering, stop disposition, and stale-result rejection.
- [x] Preserve the existing stability filter and cap visible application at one accepted update per 3.2 seconds; never promote a rejected raw hypothesis merely because it is newest.
- [x] Add failing tests for foreground-target matching, batched Windows backspaces, and the empty-audio leak watchdog.
- [x] Batch Windows backspaces into one `SendInput` call and reject live/final replacement when the originally captured target window is no longer foreground.
- [ ] Treat caret movement inside the same window as unverified: generic Windows foreground-window APIs do not prove caret ownership across Chromium, native editors, and RDP.
- [ ] Underlying ONNX inference cancellation remains unimplemented because the engine call is synchronous and non-cancellable; discard its result after stop and measure the unavoidable wait explicitly.
- [ ] Add structured diagnostic events for queue, generation, endpoint, provider, inference, disposition, stop request, reconciliation, paste, and readback/verification outcome.

## Task 8: Verify before and after on the same dataset

- [x] Run Rust unit/integration/concurrency tests using the short target path.
- [x] Run frontend build, lint, translation checks, and scoped formatting checks.
- [x] Re-run the exact 17-session Parakeet V3/DirectML final-pass matrix against the verified Track A production binary.
- [x] Validate acceptance criteria and explicitly classify each as met, not met with lower bound, or unverified.
- [ ] Run a long/repeated stress sequence and inspect diagnostics for drops, duplicates, ordering errors, and leaks.

## Task 9: Safe installation and live validation

- [x] Back up the current installed executable and record hashes before replacement.
- [x] Build the production executable and install only after all offline gates pass; preserve `D:\Apps\Handy\Data`.
- [ ] Run a live validation flow in a controlled test target, including focus change, cancellation, cursor movement, and paste failure recovery.
- [x] Confirm the deployed executable and retained diagnostics match the verified build.
- [x] Document rollback and restore instructions.

## Task 10: Review, document, commit, and push

- [ ] Request an independent code review after implementation and address findings with evidence.
- [ ] Produce the aggregate report, per-session timelines, benchmark matrix, recommendation, caveats, and reproducibility commands.
- [ ] Verify the final diff contains no private transcripts, recordings, secrets, downloaded models, or generated bulk artifacts.
- [ ] Commit with conventional messages and push the feature branch only after fresh verification.
