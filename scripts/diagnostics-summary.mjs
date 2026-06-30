#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const defaultDiagnosticsDir = path.resolve(
  __dirname,
  "..",
  "src-tauri",
  "target",
  "release",
  "Data",
  "diagnostics",
);
const diagnosticsDir = process.argv[2]
  ? path.resolve(process.argv[2])
  : defaultDiagnosticsDir;

function readEvents(filePath) {
  return fs
    .readFileSync(filePath, "utf8")
    .split(/\r?\n/)
    .filter(Boolean)
    .map((line, index) => {
      try {
        return JSON.parse(line);
      } catch (error) {
        return {
          event_type: "parse_error",
          payload: { line: index + 1, error: String(error) },
        };
      }
    });
}

function latestPayload(events, eventType) {
  return events
    .filter((event) => event.event_type === eventType)
    .map((event) => event.payload)
    .at(-1);
}

function allPayloads(events, eventType) {
  return events
    .filter((event) => event.event_type === eventType)
    .map((event) => event.payload);
}

function sum(values) {
  return values.reduce((total, value) => total + (Number(value) || 0), 0);
}

function max(values) {
  return values.length
    ? Math.max(...values.map((value) => Number(value) || 0))
    : 0;
}

function previewText(text) {
  if (!text) {
    return "";
  }
  const collapsed = String(text).replace(/\s+/g, " ").trim();
  return collapsed.length > 110 ? `${collapsed.slice(0, 107)}...` : collapsed;
}

function summarizeFile(filePath) {
  const events = readEvents(filePath);
  const start = latestPayload(events, "session_start") ?? {};
  const recordingStop = latestPayload(events, "recording_stop") ?? {};
  const history = latestPayload(events, "history_saved") ?? {};
  const paste = latestPayload(events, "paste_result") ?? {};
  const finalText =
    latestPayload(events, "final_text_ready") ??
    latestPayload(events, "final_full_transcript") ??
    latestPayload(events, "final_transcript") ??
    latestPayload(events, "final_fallback_transcript") ??
    {};
  const finalEdit = latestPayload(events, "final_reconciliation_edit") ?? {};
  const progressiveEdits = allPayloads(events, "progressive_edit");
  const progressiveSkips = allPayloads(events, "progressive_skip");
  const interimTranscripts = allPayloads(
    events,
    "progressive_interim_transcript",
  );
  const errors = events.filter((event) => event.event_type.endsWith("_error"));

  return {
    file: path.basename(filePath),
    started: events[0]?.timestamp ?? "",
    mode: recordingStop.mode ?? finalText.mode ?? "",
    model: start.selected_model ?? "",
    language: start.selected_language ?? "",
    stability: start.dictation_stability_mode ?? "",
    events: events.length,
    interimPasses: interimTranscripts.length,
    interimMsTotal: sum(
      interimTranscripts.map((payload) => payload.elapsed_ms),
    ),
    interimMsMax: max(interimTranscripts.map((payload) => payload.elapsed_ms)),
    finalMs:
      finalText.elapsed_ms ??
      latestPayload(events, "final_full_transcript")?.elapsed_ms ??
      latestPayload(events, "final_transcript")?.elapsed_ms ??
      "",
    progressiveEditBackspaces: sum(
      progressiveEdits.map((payload) => payload.backspaces),
    ),
    progressiveEditInsertions: sum(
      progressiveEdits.map((payload) => payload.insertion_chars),
    ),
    progressiveSkippedWouldBackspace: sum(
      progressiveSkips.map((payload) => payload.would_backspace_chars),
    ),
    finalBackspaces: finalEdit.backspaces ?? 0,
    finalInsertions: finalEdit.insertion_chars ?? 0,
    pasteStatus: paste.status ?? "",
    wavFile: history.wav_file ?? "",
    errors: errors.length,
    text: previewText(
      finalText.final_text ?? finalText.text ?? finalEdit.replacement_text,
    ),
  };
}

if (!fs.existsSync(diagnosticsDir)) {
  console.error(`Diagnostics directory not found: ${diagnosticsDir}`);
  process.exit(1);
}

const files = fs
  .readdirSync(diagnosticsDir)
  .filter((file) => file.endsWith(".jsonl"))
  .map((file) => path.join(diagnosticsDir, file))
  .sort();

if (files.length === 0) {
  console.log(`No diagnostic sessions found in ${diagnosticsDir}`);
  process.exit(0);
}

const summaries = files.map(summarizeFile);
console.log(`Diagnostics directory: ${diagnosticsDir}`);
console.log(`Sessions: ${summaries.length}`);
console.table(
  summaries.map((summary) => ({
    file: summary.file,
    mode: summary.mode,
    model: summary.model,
    passes: summary.interimPasses,
    interimMaxMs: summary.interimMsMax,
    finalMs: summary.finalMs,
    liveBackspaces: summary.progressiveEditBackspaces,
    skippedBackspaces: summary.progressiveSkippedWouldBackspace,
    finalBackspaces: summary.finalBackspaces,
    paste: summary.pasteStatus,
    wav: summary.wavFile,
    errors: summary.errors,
  })),
);

console.log("\nLatest text previews:");
for (const summary of summaries.slice(-10)) {
  console.log(`- ${summary.file}: ${summary.text}`);
}
