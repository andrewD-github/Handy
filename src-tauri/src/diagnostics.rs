use anyhow::Result;
use chrono::Utc;
use serde_json::{json, Value};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tauri::AppHandle;

use crate::settings::AppSettings;

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
pub(crate) struct DiagnosticSession {
    inner: Arc<DiagnosticSessionInner>,
}

struct DiagnosticSessionInner {
    id: String,
    path: PathBuf,
    started: Instant,
    sequence: Mutex<u64>,
}

impl DiagnosticSession {
    pub(crate) fn start(app: &AppHandle, generation: u64, settings: &AppSettings) -> Option<Self> {
        if !settings.diagnostic_capture_enabled {
            return None;
        }

        let directory = match diagnostics_dir(app) {
            Ok(directory) => directory,
            Err(error) => {
                log::error!("Failed to resolve diagnostics directory: {error}");
                return None;
            }
        };
        if let Err(error) = cleanup_old_diagnostics(&directory, settings.diagnostic_retention_days)
        {
            log::warn!("Failed to clean old diagnostics: {error}");
        }

        let now = Utc::now();
        let counter = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
        let id = format!("diag-{}-{counter}", now.format("%Y%m%d-%H%M%S"));
        let session = Self {
            inner: Arc::new(DiagnosticSessionInner {
                path: directory.join(format!("{id}.jsonl")),
                id,
                started: Instant::now(),
                sequence: Mutex::new(0),
            }),
        };
        session.record_json(
            "session_start",
            json!({
                "generation": generation,
                "selected_model": settings.selected_model,
                "selected_language": settings.selected_language,
                "progressive_output_mode": format!("{:?}", settings.progressive_output_mode),
                "paste_method": format!("{:?}", settings.paste_method),
                "transcribe_accelerator": format!("{:?}", settings.transcribe_accelerator),
                "ort_accelerator": format!("{:?}", settings.ort_accelerator),
            }),
        );
        Some(session)
    }

    #[cfg(test)]
    fn new_for_test(id: &str, path: PathBuf) -> Self {
        Self {
            inner: Arc::new(DiagnosticSessionInner {
                id: id.to_string(),
                path,
                started: Instant::now(),
                sequence: Mutex::new(0),
            }),
        }
    }

    #[cfg(test)]
    fn path(&self) -> &Path {
        &self.inner.path
    }

    pub(crate) fn record_json(&self, event_type: &str, payload: Value) {
        if let Err(error) = self.write_event(event_type, payload) {
            log::error!("Failed to write diagnostic event '{event_type}': {error}");
        }
    }

    fn write_event(&self, event_type: &str, payload: Value) -> Result<()> {
        let mut sequence = self.inner.sequence.lock().unwrap();
        *sequence += 1;
        if let Some(parent) = self.inner.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let event = json!({
            "session_id": self.inner.id,
            "sequence": *sequence,
            "event_type": event_type,
            "timestamp": Utc::now().to_rfc3339(),
            "elapsed_ms": self.inner.started.elapsed().as_millis(),
            "payload": payload,
        });
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.inner.path)?;
        serde_json::to_writer(&mut file, &event)?;
        file.write_all(b"\n")?;
        Ok(())
    }
}

pub(crate) fn snapshot_metadata(
    generation: u64,
    committed: &str,
    tentative: &str,
    decision: &str,
) -> Value {
    json!({
        "generation": generation,
        "committed_chars": committed.chars().count(),
        "tentative_chars": tentative.chars().count(),
        "decision": decision,
    })
}

fn diagnostics_dir(app: &AppHandle) -> Result<PathBuf> {
    Ok(crate::portable::app_data_dir(app)?.join("diagnostics"))
}

fn cleanup_old_diagnostics(directory: &Path, retention_days: u32) -> Result<()> {
    if retention_days == 0 || !directory.exists() {
        return Ok(());
    }
    let retention = std::time::Duration::from_secs(retention_days as u64 * 24 * 60 * 60);
    let now = std::time::SystemTime::now();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("jsonl") {
            continue;
        }
        let modified = match entry.metadata()?.modified() {
            Ok(modified) => modified,
            Err(_) => continue,
        };
        if now.duration_since(modified).unwrap_or_default() > retention {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::DiagnosticSession;
    use serde_json::{json, Value};
    use std::fs;

    #[test]
    fn diagnostic_session_writes_ordered_jsonl_with_session_identity() {
        let temp = tempfile::tempdir().unwrap();
        let session =
            DiagnosticSession::new_for_test("diag-test", temp.path().join("diag-test.jsonl"));

        session.record_json(
            "stream_snapshot",
            json!({"generation": 7, "committed": "hello", "tentative": " world"}),
        );
        session.record_json(
            "finalization",
            json!({"generation": 7, "final_text": "hello world"}),
        );

        let events: Vec<Value> = fs::read_to_string(session.path())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["session_id"], "diag-test");
        assert_eq!(events[0]["sequence"], 1);
        assert_eq!(events[0]["event_type"], "stream_snapshot");
        assert_eq!(events[0]["payload"]["committed"], "hello");
        assert_eq!(events[1]["sequence"], 2);
        assert_eq!(events[1]["payload"]["final_text"], "hello world");
    }

    #[test]
    fn metadata_event_contains_lengths_and_decision_without_unrelated_clipboard_data() {
        let payload = super::snapshot_metadata(7, "hello", " world", "append");

        assert_eq!(payload["generation"], 7);
        assert_eq!(payload["committed_chars"], 5);
        assert_eq!(payload["tentative_chars"], 6);
        assert_eq!(payload["decision"], "append");
        assert!(payload.get("clipboard").is_none());
    }
}
