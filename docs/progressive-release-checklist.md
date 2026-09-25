# Progressive Handy Release Checklist

This checklist is the only path from a development build to the portable daily
driver at `D:\Apps\Handy`. A successful compile alone does not authorize an
installation.

## Immutable inputs

- Baseline dataset: `D:\Apps\Handy\Data\analysis\actual-usage-20260712`
- Frozen source snapshot: `D:\Apps\Handy\Data\analysis\snapshot-20260712-122337`
- Verified v0.9.5 outputs: `D:\Apps\Handy\Data\analysis\candidate-0.9.5-progressive`
- Verified v0.9.6 refocus outputs: `D:\Apps\Handy\Data\analysis\candidate-0.9.6-refocus-resume`
- New candidate outputs: `D:\Apps\Handy\Data\analysis\candidate-0.9.7-progressive`
- Replay cohort: all 17 exact WAV/history/diagnostic joins from `manifest.json`
- Join rule: session ID, history ID, exact WAV filename, and corroborating
  timestamps; never directory order
- Ground truth status: none. History and diagnostic transcripts are observed
  Handy output, so agreement distances are not WER or CER.

## Automated gates

- [ ] Full Rust suite passes.
- [ ] Replay-tool tests pass.
- [ ] Frontend lint, typecheck, production build, and translation consistency
      pass.
- [ ] Optimized candidate build completes. If installer signing is unavailable,
      record that separately; do not describe an unsigned portable executable as
      a signed installer.
- [ ] All three candidate model files match the SHA-256 values pinned in Handy's
      bundled catalog.
- [ ] All 17 real-use WAVs complete through the production streaming worker in
      simulated real time on the selected accelerator.

## Replay acceptance

The old 0.8.3 observed p95 values were 7,402 ms first visible text, 13,792 ms
maximum visible update gap, and 4,446 ms stop-to-final. A release candidate must
meet all of these gates on the 17-session cohort:

- [ ] p95 first visible display text below 3,000 ms.
- [ ] p95 maximum visible display update gap below 4,000 ms.
- [ ] p95 stop-to-final below 1,000 ms.
- [ ] zero enacted live-display retractions or backspaces.
- [ ] no final disagreement larger than four token edits on the frozen cohort;
      final disagreements are recorded and are not paragraph-rewritten.
- [ ] p95 final appended/different text no more than five token edits.
- [ ] No claim that distance to saved history measures correctness.
- [ ] Materially different candidate/history outputs are queued for blinded
      human adjudication if an accuracy choice remains.

## Live behavior gates

Run the isolated portable candidate, not the installed executable.

- [ ] ChatGPT: text visibly grows inside the active prompt during dictation.
- [ ] With refocus resume disabled: click Submit before F4, then press F4; no
      old-session tail appears in the new prompt.
- [ ] With refocus resume enabled: click away from the dictation prompt; direct
      insertion pauses while the Handy live overlay continues.
- [ ] With refocus resume enabled: click back into the original target; the
      accumulated append-only suffix appears exactly once and streaming continues.
- [ ] With refocus resume enabled: stop while another target is active; no final
      tail is inserted into that target.
- [ ] With refocus resume enabled: verify and record the accepted limitation
      that a reused Chromium/WebView prompt may receive pending text after Submit.
- [ ] Mouse movement alone does not suspend or invalidate live insertion.
- [ ] With refocus resume disabled, a pointer click during a session prevents
      all later prompt insertion.
- [ ] Stop with F4 appends at most the measured final tail once.
- [ ] Cancel produces no late text.
- [ ] Rapid F4 restart makes every previous-generation update stale.
- [ ] Switching focus prevents edits in both the old and new target.
- [ ] A native editor receives progressive text and finalizes once.
- [ ] Overlay mode retains official Handy behavior.
- [ ] Text, image, and rich clipboard contents survive fallback paste behavior.
- [ ] The blue token theme renders legibly in the main window and overlay.

## Install and rollback

- [ ] Stop the installed Handy process.
- [ ] Record the installed executable hash.
- [ ] Create a timestamped backup of the executable and every runtime DLL that
      will be replaced.
- [ ] Do not replace or recreate `D:\Apps\Handy\Data`.
- [ ] Copy the winning catalog-pinned streaming model into
      `D:\Apps\Handy\Data\models` and verify its hash in place.
- [ ] Preserve the old custom setting during migration, select direct-prompt
      output, and select the winning streaming model.
- [ ] Copy the candidate executable and matching native runtime DLLs.
- [ ] Start Handy and repeat the ChatGPT residual-text test against the installed
      surface.
- [ ] Confirm settings, history, recordings, and the model persist after restart.
- [ ] If any installed-surface gate fails, stop Handy, restore the timestamped
      executable/runtime backup, and leave `Data` untouched.

## Future upstream upgrade

1. Fetch and verify the new stable upstream tag.
2. Preview the merge and inventory textual plus semantic overlap with the
   custom hooks listed in `docs/upstream-integration.md`.
3. Merge the complete tag. Resolve only the documented hook points; never
   restore an old batch scheduler or copy an old transcription manager
   wholesale.
4. Regenerate bindings, run every automated gate, rebuild the isolated
   candidate, and replay the same frozen cohort.
5. Add new exact-use sessions to a new frozen cohort rather than modifying the
   historical baseline in place.
