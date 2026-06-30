use anyhow::Result;
use chrono::Utc;
use serde::Serialize;
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
pub struct DiagnosticSession {
    inner: Arc<DiagnosticSessionInner>,
}

struct DiagnosticSessionInner {
    id: String,
    path: PathBuf,
    started: Instant,
    sequence: Mutex<u64>,
}

#[derive(Debug, Serialize)]
pub struct DiagnosticStart<'a> {
    pub binding_id: &'a str,
    pub post_process: bool,
    pub selected_model: &'a str,
    pub selected_language: &'a str,
    pub dictation_stability_mode: String,
    pub paste_method: String,
    pub voice_finish_trigger_enabled: bool,
}

impl DiagnosticSession {
    pub fn start(
        app: &AppHandle,
        binding_id: &str,
        post_process: bool,
        settings: &AppSettings,
    ) -> Option<Self> {
        if !settings.diagnostic_capture_enabled {
            return None;
        }

        let diagnostics_dir = match diagnostics_dir(app) {
            Ok(path) => path,
            Err(err) => {
                log::error!("Failed to resolve diagnostics directory: {}", err);
                return None;
            }
        };

        if let Err(err) =
            cleanup_old_diagnostics(&diagnostics_dir, settings.diagnostic_retention_days)
        {
            log::warn!("Failed to clean old diagnostic files: {}", err);
        }

        let started_at = Utc::now();
        let counter = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
        let id = format!("diag-{}-{}", started_at.format("%Y%m%d-%H%M%S"), counter);
        let path = diagnostics_dir.join(format!("{}.jsonl", id));

        let session = Self {
            inner: Arc::new(DiagnosticSessionInner {
                id,
                path,
                started: Instant::now(),
                sequence: Mutex::new(0),
            }),
        };

        session.record(
            "session_start",
            DiagnosticStart {
                binding_id,
                post_process,
                selected_model: &settings.selected_model,
                selected_language: &settings.selected_language,
                dictation_stability_mode: format!("{:?}", settings.dictation_stability_mode),
                paste_method: format!("{:?}", settings.paste_method),
                voice_finish_trigger_enabled: settings.voice_finish_trigger_enabled,
            },
        );

        Some(session)
    }

    #[cfg(test)]
    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    pub fn record<T: Serialize>(&self, event_type: &str, payload: T) {
        let payload_value = match serde_json::to_value(payload) {
            Ok(value) => value,
            Err(err) => {
                log::error!(
                    "Failed to serialize diagnostic event '{}': {}",
                    event_type,
                    err
                );
                return;
            }
        };

        if let Err(err) = self.write_event(event_type, payload_value) {
            log::error!("Failed to write diagnostic event '{}': {}", event_type, err);
        }
    }

    pub fn record_json(&self, event_type: &str, payload: Value) {
        if let Err(err) = self.write_event(event_type, payload) {
            log::error!("Failed to write diagnostic event '{}': {}", event_type, err);
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

pub fn diagnostics_dir(app: &AppHandle) -> Result<PathBuf> {
    Ok(crate::portable::app_data_dir(app)?.join("diagnostics"))
}

fn cleanup_old_diagnostics(diagnostics_dir: &Path, retention_days: u32) -> Result<()> {
    if retention_days == 0 || !diagnostics_dir.exists() {
        return Ok(());
    }

    let retention = std::time::Duration::from_secs(retention_days as u64 * 24 * 60 * 60);
    let now = std::time::SystemTime::now();

    for entry in fs::read_dir(diagnostics_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }

        let metadata = entry.metadata()?;
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        if now.duration_since(modified).unwrap_or_default() > retention {
            fs::remove_file(&path)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::time::{Duration, SystemTime};

    #[test]
    fn diagnostic_session_writes_jsonl_events_with_sequence_and_payload() {
        let temp = tempfile::tempdir().unwrap();
        let session = DiagnosticSession {
            inner: Arc::new(DiagnosticSessionInner {
                id: "diag-test".to_string(),
                path: temp.path().join("diag-test.jsonl"),
                started: Instant::now(),
                sequence: Mutex::new(0),
            }),
        };

        session.record_json("chunk", json!({ "text": "hello", "elapsed_ms": 123 }));
        session.record_json("edit", json!({ "backspaces": 3, "insertion_chars": 9 }));

        let contents = fs::read_to_string(session.path()).unwrap();
        let events: Vec<Value> = contents
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();

        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["session_id"], "diag-test");
        assert_eq!(events[0]["sequence"], 1);
        assert_eq!(events[0]["event_type"], "chunk");
        assert_eq!(events[0]["payload"]["text"], "hello");
        assert_eq!(events[1]["sequence"], 2);
        assert_eq!(events[1]["payload"]["backspaces"], 3);
    }

    #[test]
    fn cleanup_old_diagnostics_removes_only_expired_jsonl_files() {
        let temp = tempfile::tempdir().unwrap();
        let old_file = temp.path().join("old.jsonl");
        let fresh_file = temp.path().join("fresh.jsonl");
        let other_file = temp.path().join("old.txt");

        fs::write(&old_file, "{}\n").unwrap();
        fs::write(&fresh_file, "{}\n").unwrap();
        fs::write(&other_file, "keep").unwrap();

        let old_time = filetime::FileTime::from_system_time(
            SystemTime::now() - Duration::from_secs(3 * 24 * 60 * 60),
        );
        filetime::set_file_mtime(&old_file, old_time).unwrap();
        filetime::set_file_mtime(&other_file, old_time).unwrap();

        cleanup_old_diagnostics(temp.path(), 1).unwrap();

        assert!(!old_file.exists());
        assert!(fresh_file.exists());
        assert!(other_file.exists());
    }
}
