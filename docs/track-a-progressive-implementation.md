# Progressive Dictation Track A

## Installed outcome

Track A preserves progressive text in the user's active target application. It does not move live text into the Handy overlay.

The installed build:

- preserves the existing stability decision before an interim hypothesis can become visible;
- displays the first accepted hypothesis immediately;
- coalesces later accepted hypotheses and applies at most one every 3.2 seconds;
- generation-tags interim work and rejects stale or post-stop results;
- synchronizes stop with the transition into interim inference;
- batches Windows Backspace press/release events into one `SendInput` call;
- verifies the originally captured foreground window on the main thread, immediately before Windows deletion, and again before insertion;
- requests a clean stop after five minutes without retained audio, including sessions that never receive audio;
- records coalescing, focus-guard, stop-wait, discarded-result, generation, and watchdog diagnostics.

## Offline acceptance evidence

- Rust: 96 tests passed.
- Frontend: TypeScript/Vite production build passed.
- Lint: passed.
- Translations: all 19 non-reference languages complete.
- Same-WAV replay: 17/17 Parakeet V3/DirectML sessions completed.
- Replay latency: p50 1.763 seconds; p95 3.630 seconds; maximum 3.828 seconds.
- Final replay text: 17/17 identical to the previous V3/DirectML replay.
- Display-policy simulation: applying only stability-accepted hypotheses with a 3.2-second minimum interval reduced the saved-hypothesis deletion proxy from 824 to 713 words; the maximum single rewrite remained 70 words.

The replay text comparison is determinism/agreement evidence, not accuracy evidence. No trustworthy corrected references exist, so no WER or CER is claimed.

## Known limits

- The current ONNX inference call cannot be interrupted after it enters the engine. Stop prevents new interim work, discards a late interim result, measures the wait, and then runs the final pass.
- Foreground-window identity cannot prove caret ownership inside the same Chromium/native/RDP window. The build prevents cross-window replacement but does not claim universal same-window caret protection.
- Full-window Parakeet remains intrinsically capable of hypothesis churn. Track A does not increase the accepted visible cadence and does not introduce the empirically rejected two-pass word-lock policy.
- End-to-end microphone, focus-change, and target-application behavior requires controlled live dictation; same-WAV headless replay cannot exercise synthetic input delivery.

## Installation and rollback

- Installed executable SHA-256: `C2F14700485927893AC6B4F7D303F70D6341B2B8D31A1A931E8356D221FA3580`
- Previous executable backup: `D:\Apps\Handy\backups\track-a-20260712-192300\handy.exe`
- Previous executable SHA-256: `21C8EC7B761A9EFCDBF6D935744E02E984F7366442786DA8031695C2056CD9EA`

Rollback requires stopping Handy, copying the backed-up executable over `D:\Apps\Handy\handy.exe`, verifying the backup hash, and restarting Handy. The data directory must not be replaced.
