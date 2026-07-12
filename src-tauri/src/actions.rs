#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use crate::apple_intelligence;
use crate::audio_feedback::{play_feedback_sound, play_feedback_sound_blocking, SoundType};
use crate::audio_toolkit::{is_microphone_access_denied, is_no_input_device_error};
use crate::diagnostics::DiagnosticSession;
#[cfg(target_os = "windows")]
use crate::input::ensure_target_window;
use crate::input::{foreground_window_id, target_window_matches};
use crate::managers::audio::AudioRecordingManager;
use crate::managers::history::HistoryManager;
use crate::managers::transcription::TranscriptionManager;
use crate::settings::{
    get_settings, AppSettings, DictationStabilityMode, APPLE_INTELLIGENCE_PROVIDER_ID,
};
use crate::shortcut;
use crate::tray::{change_tray_icon, TrayIconState};
use crate::utils::{
    self, show_processing_overlay, show_recording_overlay, show_transcribing_overlay,
};
use crate::TranscriptionCoordinator;
use ferrous_opencc::{config::BuiltinConfig, OpenCC};
use log::{debug, error, info, warn};
use once_cell::sync::Lazy;
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use tauri::Manager;
use tauri::{AppHandle, Emitter};

#[derive(Clone, serde::Serialize)]
struct RecordingErrorEvent {
    error_type: String,
    detail: Option<String>,
}

/// Drop guard that notifies the [`TranscriptionCoordinator`] when the
/// transcription pipeline finishes — whether it completes normally or panics.
struct FinishGuard(AppHandle);
impl Drop for FinishGuard {
    fn drop(&mut self) {
        if let Some(c) = self.0.try_state::<TranscriptionCoordinator>() {
            c.notify_processing_finished();
        }
    }
}

const PROGRESSIVE_CHUNK_INTERVAL: Duration = Duration::from_secs(2);
const PROGRESSIVE_VISIBLE_UPDATE_INTERVAL_MS: u64 = 3_200;
const PROGRESSIVE_EMPTY_AUDIO_WATCHDOG_MS: u64 = 5 * 60 * 1_000;
const PROGRESSIVE_MIN_SAMPLES: usize = 16_000;
const PROGRESSIVE_INTERIM_CHURN_GUARD_MIN_CHARS: usize = 80;
const PROGRESSIVE_INTERIM_CHURN_GUARD_MAX_BACKSPACES: usize = 60;
const PROGRESSIVE_INTERIM_CHURN_GUARD_MIN_PREFIX: usize = 40;

struct ProgressiveDisplayGate<T> {
    latest_generation_seen: u64,
    last_applied_elapsed_ms: Option<u64>,
    pending: Option<(u64, T)>,
    stopped: bool,
}

impl<T> Default for ProgressiveDisplayGate<T> {
    fn default() -> Self {
        Self {
            latest_generation_seen: 0,
            last_applied_elapsed_ms: None,
            pending: None,
            stopped: false,
        }
    }
}

impl<T> ProgressiveDisplayGate<T> {
    fn offer(&mut self, generation: u64, elapsed_ms: u64, value: T) -> Option<(u64, T)> {
        if self.stopped || generation <= self.latest_generation_seen {
            return None;
        }

        self.latest_generation_seen = generation;
        self.pending = Some((generation, value));
        self.poll(elapsed_ms)
    }

    fn poll(&mut self, elapsed_ms: u64) -> Option<(u64, T)> {
        if self.stopped || self.pending.is_none() {
            return None;
        }

        let due = self.last_applied_elapsed_ms.is_none_or(|last| {
            elapsed_ms.saturating_sub(last) >= PROGRESSIVE_VISIBLE_UPDATE_INTERVAL_MS
        });
        if !due {
            return None;
        }

        self.last_applied_elapsed_ms = Some(elapsed_ms);
        self.pending.take()
    }

    fn stop(&mut self) {
        self.stopped = true;
        self.pending = None;
    }
}

#[derive(Default)]
struct ProgressiveLeakWatchdog {
    last_audio_elapsed_ms: Option<u64>,
    fired: bool,
}

impl ProgressiveLeakWatchdog {
    fn observe(&mut self, elapsed_ms: u64, sample_count: usize) -> bool {
        if sample_count > 0 {
            self.last_audio_elapsed_ms = Some(elapsed_ms);
            self.fired = false;
            return false;
        }

        if self.fired {
            return false;
        }
        let last_audio_elapsed_ms = self.last_audio_elapsed_ms.unwrap_or(0);
        if elapsed_ms.saturating_sub(last_audio_elapsed_ms) < PROGRESSIVE_EMPTY_AUDIO_WATCHDOG_MS {
            return false;
        }

        self.fired = true;
        true
    }
}

struct ProgressiveSession {
    committed_text: Mutex<String>,
    locked_prefix_chars: Mutex<usize>,
    pending_audio: Mutex<Vec<f32>>,
    all_audio: Mutex<Vec<f32>>,
    diagnostic: Option<DiagnosticSession>,
    in_flight: Mutex<bool>,
    idle: Condvar,
    stopping: AtomicBool,
    started_at: Instant,
    generation: AtomicU64,
    display_gate: Mutex<ProgressiveDisplayGate<ProgressiveInterimUpdate>>,
    target_window_id: Option<isize>,
    leak_watchdog: Mutex<ProgressiveLeakWatchdog>,
}

impl ProgressiveSession {
    fn new(diagnostic: Option<DiagnosticSession>, target_window_id: Option<isize>) -> Self {
        Self {
            committed_text: Mutex::new(String::new()),
            locked_prefix_chars: Mutex::new(0),
            pending_audio: Mutex::new(Vec::new()),
            all_audio: Mutex::new(Vec::new()),
            diagnostic,
            in_flight: Mutex::new(false),
            idle: Condvar::new(),
            stopping: AtomicBool::new(false),
            started_at: Instant::now(),
            generation: AtomicU64::new(0),
            display_gate: Mutex::new(ProgressiveDisplayGate::default()),
            target_window_id,
            leak_watchdog: Mutex::new(ProgressiveLeakWatchdog::default()),
        }
    }

    fn set_in_flight(&self, value: bool) {
        let mut in_flight = self.in_flight.lock().unwrap();
        *in_flight = value;
        if !value {
            self.idle.notify_all();
        }
    }

    fn try_begin_inference(&self) -> bool {
        let mut in_flight = self.in_flight.lock().unwrap();
        if self.stopping.load(Ordering::Acquire) {
            return false;
        }
        *in_flight = true;
        true
    }

    fn begin_stopping_and_wait(&self) {
        let mut in_flight = self.in_flight.lock().unwrap();
        self.stopping.store(true, Ordering::Release);
        self.stop_visible_updates();
        while *in_flight {
            in_flight = self.idle.wait(in_flight).unwrap();
        }
    }

    fn push_audio(&self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        self.pending_audio
            .lock()
            .unwrap()
            .extend_from_slice(samples);
        self.all_audio.lock().unwrap().extend_from_slice(samples);
    }

    fn take_pending_audio(&self) -> Vec<f32> {
        std::mem::take(&mut *self.pending_audio.lock().unwrap())
    }

    fn all_audio(&self) -> Vec<f32> {
        self.all_audio.lock().unwrap().clone()
    }

    fn audio_sample_count(&self) -> usize {
        self.all_audio.lock().unwrap().len()
    }

    fn diagnostic(&self) -> Option<DiagnosticSession> {
        self.diagnostic.clone()
    }

    fn take_ready_audio(&self) -> Option<Vec<f32>> {
        let mut pending = self.pending_audio.lock().unwrap();
        if pending.len() >= PROGRESSIVE_MIN_SAMPLES {
            Some(std::mem::take(&mut *pending))
        } else {
            None
        }
    }

    fn committed_text(&self) -> String {
        self.committed_text.lock().unwrap().clone()
    }

    fn replace_committed_text(&self, text: String) {
        *self.committed_text.lock().unwrap() = text;
    }

    fn locked_prefix_chars(&self) -> usize {
        *self.locked_prefix_chars.lock().unwrap()
    }

    fn replace_locked_prefix_chars(&self, chars: usize) {
        *self.locked_prefix_chars.lock().unwrap() = chars;
    }

    fn next_generation(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn offer_visible_update(
        &self,
        generation: u64,
        update: ProgressiveInterimUpdate,
    ) -> Option<(u64, ProgressiveInterimUpdate)> {
        let elapsed_ms = self.started_at.elapsed().as_millis() as u64;
        self.display_gate
            .lock()
            .unwrap()
            .offer(generation, elapsed_ms, update)
    }

    fn stop_visible_updates(&self) {
        self.display_gate.lock().unwrap().stop();
    }

    fn poll_visible_update(&self) -> Option<(u64, ProgressiveInterimUpdate)> {
        let elapsed_ms = self.started_at.elapsed().as_millis() as u64;
        self.display_gate.lock().unwrap().poll(elapsed_ms)
    }

    fn target_is_foreground(&self) -> bool {
        #[cfg(target_os = "windows")]
        {
            target_window_matches(self.target_window_id, foreground_window_id())
        }

        #[cfg(not(target_os = "windows"))]
        {
            true
        }
    }

    fn observe_audio_for_leak(&self, sample_count: usize) -> bool {
        self.leak_watchdog
            .lock()
            .unwrap()
            .observe(self.started_at.elapsed().as_millis() as u64, sample_count)
    }
}

static PROGRESSIVE_SESSIONS: Lazy<Mutex<HashMap<String, Arc<ProgressiveSession>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static DIAGNOSTIC_SESSIONS: Lazy<Mutex<HashMap<String, DiagnosticSession>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

// Shortcut Action Trait
pub trait ShortcutAction: Send + Sync {
    fn start(&self, app: &AppHandle, binding_id: &str, shortcut_str: &str);
    fn stop(&self, app: &AppHandle, binding_id: &str, shortcut_str: &str);
}

// Transcribe Action
struct TranscribeAction {
    post_process: bool,
}

/// Field name for structured output JSON schema
const TRANSCRIPTION_FIELD: &str = "transcription";

/// Strip invisible Unicode characters that some LLMs may insert
fn strip_invisible_chars(s: &str) -> String {
    s.replace(['\u{200B}', '\u{200C}', '\u{200D}', '\u{FEFF}'], "")
}

/// Build a system prompt from the user's prompt template.
/// Removes `${output}` placeholder since the transcription is sent as the user message.
fn build_system_prompt(prompt_template: &str) -> String {
    prompt_template.replace("${output}", "").trim().to_string()
}

fn is_blank_transcription(transcription: &str) -> bool {
    transcription.trim().is_empty()
}

async fn post_process_transcription(settings: &AppSettings, transcription: &str) -> Option<String> {
    if is_blank_transcription(transcription) {
        debug!("Post-processing skipped because the transcription is empty");
        return None;
    }

    let provider = match settings.active_post_process_provider().cloned() {
        Some(provider) => provider,
        None => {
            debug!("Post-processing enabled but no provider is selected");
            return None;
        }
    };

    let model = settings
        .post_process_models
        .get(&provider.id)
        .cloned()
        .unwrap_or_default();

    if model.trim().is_empty() {
        debug!(
            "Post-processing skipped because provider '{}' has no model configured",
            provider.id
        );
        return None;
    }

    let selected_prompt_id = match &settings.post_process_selected_prompt_id {
        Some(id) => id.clone(),
        None => {
            debug!("Post-processing skipped because no prompt is selected");
            return None;
        }
    };

    let prompt = match settings
        .post_process_prompts
        .iter()
        .find(|prompt| prompt.id == selected_prompt_id)
    {
        Some(prompt) => prompt.prompt.clone(),
        None => {
            debug!(
                "Post-processing skipped because prompt '{}' was not found",
                selected_prompt_id
            );
            return None;
        }
    };

    if prompt.trim().is_empty() {
        debug!("Post-processing skipped because the selected prompt is empty");
        return None;
    }

    debug!(
        "Starting LLM post-processing with provider '{}' (model: {})",
        provider.id, model
    );

    let api_key = settings
        .post_process_api_keys
        .get(&provider.id)
        .cloned()
        .unwrap_or_default();

    let (reasoning_effort, reasoning) = match provider.id.as_str() {
        "custom" => (Some("none".to_string()), None),
        "openrouter" => (
            None,
            Some(crate::llm_client::ReasoningConfig {
                effort: Some("none".to_string()),
                exclude: Some(true),
            }),
        ),
        _ => (None, None),
    };

    if provider.supports_structured_output {
        debug!("Using structured outputs for provider '{}'", provider.id);

        let system_prompt = build_system_prompt(&prompt);
        let user_content = transcription.to_string();

        // Handle Apple Intelligence separately since it uses native Swift APIs
        if provider.id == APPLE_INTELLIGENCE_PROVIDER_ID {
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            {
                if !apple_intelligence::check_apple_intelligence_availability() {
                    debug!(
                        "Apple Intelligence selected but not currently available on this device"
                    );
                    return None;
                }

                let token_limit = model.trim().parse::<i32>().unwrap_or(0);
                return match apple_intelligence::process_text_with_system_prompt(
                    &system_prompt,
                    &user_content,
                    token_limit,
                ) {
                    Ok(result) => {
                        if result.trim().is_empty() {
                            debug!("Apple Intelligence returned an empty response");
                            None
                        } else {
                            let result = strip_invisible_chars(&result);
                            debug!(
                                "Apple Intelligence post-processing succeeded. Output length: {} chars",
                                result.len()
                            );
                            Some(result)
                        }
                    }
                    Err(err) => {
                        error!("Apple Intelligence post-processing failed: {}", err);
                        None
                    }
                };
            }

            #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
            {
                debug!("Apple Intelligence provider selected on unsupported platform");
                return None;
            }
        }

        // Define JSON schema for transcription output
        let json_schema = serde_json::json!({
            "type": "object",
            "properties": {
                (TRANSCRIPTION_FIELD): {
                    "type": "string",
                    "description": "The cleaned and processed transcription text"
                }
            },
            "required": [TRANSCRIPTION_FIELD],
            "additionalProperties": false
        });

        match crate::llm_client::send_chat_completion_with_schema(
            &provider,
            api_key.clone(),
            &model,
            user_content,
            Some(system_prompt),
            Some(json_schema),
            reasoning_effort.clone(),
            reasoning.clone(),
        )
        .await
        {
            Ok(Some(content)) => {
                // Parse the JSON response to extract the transcription field
                match serde_json::from_str::<serde_json::Value>(&content) {
                    Ok(json) => {
                        if let Some(transcription_value) =
                            json.get(TRANSCRIPTION_FIELD).and_then(|t| t.as_str())
                        {
                            let result = strip_invisible_chars(transcription_value);
                            debug!(
                                "Structured output post-processing succeeded for provider '{}'. Output length: {} chars",
                                provider.id,
                                result.len()
                            );
                            return Some(result);
                        } else {
                            error!("Structured output response missing 'transcription' field");
                            return Some(strip_invisible_chars(&content));
                        }
                    }
                    Err(e) => {
                        error!(
                            "Failed to parse structured output JSON: {}. Returning raw content.",
                            e
                        );
                        return Some(strip_invisible_chars(&content));
                    }
                }
            }
            Ok(None) => {
                error!("LLM API response has no content");
                return None;
            }
            Err(e) => {
                warn!(
                    "Structured output failed for provider '{}': {}. Falling back to legacy mode.",
                    provider.id, e
                );
                // Fall through to legacy mode below
            }
        }
    }

    // Legacy mode: Replace ${output} variable in the prompt with the actual text
    let processed_prompt = prompt.replace("${output}", transcription);
    debug!("Processed prompt length: {} chars", processed_prompt.len());

    match crate::llm_client::send_chat_completion(
        &provider,
        api_key,
        &model,
        processed_prompt,
        reasoning_effort,
        reasoning,
    )
    .await
    {
        Ok(Some(content)) => {
            let content = strip_invisible_chars(&content);
            debug!(
                "LLM post-processing succeeded for provider '{}'. Output length: {} chars",
                provider.id,
                content.len()
            );
            Some(content)
        }
        Ok(None) => {
            error!("LLM API response has no content");
            None
        }
        Err(e) => {
            error!(
                "LLM post-processing failed for provider '{}': {}. Falling back to original transcription.",
                provider.id,
                e
            );
            None
        }
    }
}

async fn maybe_convert_chinese_variant(
    settings: &AppSettings,
    transcription: &str,
) -> Option<String> {
    // Check if language is set to Simplified or Traditional Chinese
    let is_simplified = settings.selected_language == "zh-Hans";
    let is_traditional = settings.selected_language == "zh-Hant";

    if !is_simplified && !is_traditional {
        debug!("selected_language is not Simplified or Traditional Chinese; skipping translation");
        return None;
    }

    debug!(
        "Starting Chinese translation using OpenCC for language: {}",
        settings.selected_language
    );

    // Use OpenCC to convert based on selected language
    let config = if is_simplified {
        // Convert Traditional Chinese to Simplified Chinese
        BuiltinConfig::Tw2sp
    } else {
        // Convert Simplified Chinese to Traditional Chinese
        BuiltinConfig::S2tw
    };

    match OpenCC::from_config(config) {
        Ok(converter) => {
            let converted = converter.convert(transcription);
            debug!(
                "OpenCC translation completed. Input length: {}, Output length: {}",
                transcription.len(),
                converted.len()
            );
            Some(converted)
        }
        Err(e) => {
            error!("Failed to initialize OpenCC converter: {}. Falling back to original transcription.", e);
            None
        }
    }
}

pub(crate) struct ProcessedTranscription {
    pub post_processed_text: Option<String>,
    pub post_process_prompt: Option<String>,
}

pub(crate) async fn process_transcription_output(
    app: &AppHandle,
    transcription: &str,
    post_process: bool,
) -> ProcessedTranscription {
    let settings = get_settings(app);
    let mut final_text = transcription.to_string();
    let mut post_processed_text: Option<String> = None;
    let mut post_process_prompt: Option<String> = None;

    if let Some(converted_text) = maybe_convert_chinese_variant(&settings, transcription).await {
        final_text = converted_text;
    }

    final_text = cleanup_final_text_for_paste(&final_text, &settings);

    if post_process {
        if let Some(processed_text) = post_process_transcription(&settings, &final_text).await {
            post_processed_text = Some(processed_text.clone());

            if let Some(prompt_id) = &settings.post_process_selected_prompt_id {
                if let Some(prompt) = settings
                    .post_process_prompts
                    .iter()
                    .find(|prompt| &prompt.id == prompt_id)
                {
                    post_process_prompt = Some(prompt.prompt.clone());
                }
            }
        }
    } else if final_text != transcription {
        post_processed_text = Some(final_text.clone());
    }

    ProcessedTranscription {
        post_processed_text,
        post_process_prompt,
    }
}

fn final_progressive_replacement(committed: &str, final_text: &str) -> Option<String> {
    let final_text = final_text.trim();
    if final_text.is_empty() || final_text == committed.trim() {
        None
    } else {
        Some(final_text.to_string())
    }
}

fn live_progressive_enabled_for_mode(mode: DictationStabilityMode) -> bool {
    !matches!(mode, DictationStabilityMode::FinalOnly)
}

fn common_prefix_chars(left: &str, right: &str) -> usize {
    left.chars()
        .zip(right.chars())
        .take_while(|(left_char, right_char)| left_char == right_char)
        .count()
}

fn char_to_byte_index(text: &str, char_count: usize) -> usize {
    if char_count == 0 {
        return 0;
    }

    text.char_indices()
        .nth(char_count)
        .map(|(index, _)| index)
        .unwrap_or(text.len())
}

fn last_sentence_boundary_before_or_at(text: &str, max_chars: usize) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let max_chars = max_chars.min(chars.len());
    let mut boundary = 0;

    for index in 0..max_chars {
        if matches!(chars[index], '.' | '!' | '?') {
            let mut end = index + 1;
            while end < max_chars && chars[end].is_whitespace() {
                end += 1;
            }
            boundary = end;
        }
    }

    boundary
}

fn locked_sentence_count(text: &str, locked_prefix_chars: usize) -> usize {
    text.chars()
        .take(locked_prefix_chars)
        .filter(|c| matches!(c, '.' | '!' | '?'))
        .count()
}

fn progressive_edit_stats(previous: &str, replacement: &str) -> (usize, usize, usize) {
    let shared_prefix_chars = common_prefix_chars(previous, replacement);
    let backspace_chars = previous.chars().count().saturating_sub(shared_prefix_chars);
    let insertion_chars = replacement
        .chars()
        .count()
        .saturating_sub(shared_prefix_chars);

    (shared_prefix_chars, backspace_chars, insertion_chars)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProgressiveInterimUpdate {
    text: String,
    locked_prefix_chars: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProgressiveOverlayDiagnostics {
    status: String,
    locked_sentences: usize,
    locked_chars: usize,
    backspaces: usize,
    insertion_chars: usize,
    transcript_chars: usize,
    elapsed_ms: u128,
}

fn progressive_overlay_diagnostics(
    status: &str,
    committed: &str,
    replacement: Option<&str>,
    locked_prefix_chars: usize,
    elapsed: Duration,
) -> ProgressiveOverlayDiagnostics {
    let replacement = replacement.unwrap_or(committed);
    let (_, backspaces, insertion_chars) = progressive_edit_stats(committed, replacement);

    ProgressiveOverlayDiagnostics {
        status: status.to_string(),
        locked_sentences: locked_sentence_count(replacement, locked_prefix_chars),
        locked_chars: locked_prefix_chars,
        backspaces,
        insertion_chars,
        transcript_chars: replacement.chars().count(),
        elapsed_ms: elapsed.as_millis(),
    }
}

fn emit_progressive_overlay_diagnostics(
    app: &AppHandle,
    diagnostics: ProgressiveOverlayDiagnostics,
) {
    if let Some(overlay_window) = app.get_webview_window("recording_overlay") {
        let _ = overlay_window.emit("progressive-diagnostics", diagnostics);
    }
}

#[derive(Debug, PartialEq, Eq)]
struct VoiceFinishMatch {
    cleaned_text: String,
    matched_phrase: String,
}

fn normalize_voice_finish_text(text: &str) -> String {
    let mut normalized = String::new();
    let mut previous_was_space = true;

    for ch in text.chars().flat_map(|ch| ch.to_lowercase()) {
        if ch.is_alphanumeric() {
            normalized.push(ch);
            previous_was_space = false;
        } else if !previous_was_space {
            normalized.push(' ');
            previous_was_space = true;
        }
    }

    normalized.trim().to_string()
}

fn remove_tail_words(text: &str, word_count: usize) -> String {
    if word_count == 0 {
        return text.trim().to_string();
    }

    let mut word_starts = Vec::new();
    let mut in_word = false;

    for (index, ch) in text.char_indices() {
        if ch.is_alphanumeric() {
            if !in_word {
                word_starts.push(index);
                in_word = true;
            }
        } else {
            in_word = false;
        }
    }

    if word_starts.len() < word_count {
        return String::new();
    }

    let remove_from = word_starts[word_starts.len() - word_count];
    cleanup_tail_removed_text(&text[..remove_from])
}

fn cleanup_tail_removed_text(text: &str) -> String {
    let trimmed = text
        .trim_end()
        .trim_end_matches(|ch| matches!(ch, ',' | ';' | ':'))
        .trim_end();

    let Some((last_index, last_char)) = trimmed.char_indices().last() else {
        return String::new();
    };

    if matches!(last_char, '.' | '!' | '?') {
        return format!("{}{}", trimmed[..last_index].trim_end(), last_char);
    }

    trimmed.to_string()
}

fn built_in_voice_finish_variants() -> Vec<String> {
    vec![
        "finish dictate".to_string(),
        "end dictate".to_string(),
        "finish dictatation".to_string(),
        "end dictatation".to_string(),
    ]
}

fn detect_voice_finish_trigger(
    text: &str,
    phrases: &[String],
    variants: &[String],
) -> Option<VoiceFinishMatch> {
    let mut remaining_normalized = normalize_voice_finish_text(text);
    if remaining_normalized.is_empty() {
        return None;
    }

    let built_in_variants = built_in_voice_finish_variants();
    let candidates: Vec<(String, String, usize)> = phrases
        .iter()
        .chain(variants.iter())
        .chain(built_in_variants.iter())
        .filter_map(|phrase| {
            let normalized_phrase = normalize_voice_finish_text(phrase);
            if normalized_phrase.is_empty() {
                return None;
            }

            Some((
                phrase.trim().to_string(),
                normalized_phrase.clone(),
                normalized_phrase.split_whitespace().count(),
            ))
        })
        .collect();

    if candidates.is_empty() {
        return None;
    }

    let mut words_to_remove = 0;
    let mut matched_phrases = Vec::new();

    loop {
        let Some((matched_phrase, phrase_word_count)) = candidates
            .iter()
            .filter_map(|(phrase, normalized_phrase, phrase_word_count)| {
                let matches_tail = remaining_normalized == *normalized_phrase
                    || remaining_normalized.ends_with(&format!(" {}", normalized_phrase));

                if matches_tail {
                    Some((phrase.clone(), *phrase_word_count, normalized_phrase.len()))
                } else {
                    None
                }
            })
            .max_by_key(|(_, _, normalized_len)| *normalized_len)
            .map(|(phrase, word_count, _)| (phrase, word_count))
        else {
            break;
        };

        words_to_remove += phrase_word_count;
        matched_phrases.push(matched_phrase);
        remaining_normalized = remove_tail_words(&remaining_normalized, phrase_word_count);

        if remaining_normalized.is_empty() {
            break;
        }
    }

    if words_to_remove == 0 {
        return None;
    }

    matched_phrases.reverse();
    Some(VoiceFinishMatch {
        cleaned_text: remove_tail_words(text, words_to_remove),
        matched_phrase: matched_phrases.join(" + "),
    })
}

fn cleanup_final_text_for_paste(text: &str, settings: &AppSettings) -> String {
    if !settings.voice_finish_trigger_enabled {
        return text.to_string();
    }

    if let Some(matched) = detect_voice_finish_trigger(
        text,
        &settings.voice_finish_phrases,
        &settings.voice_finish_phrase_variants,
    ) {
        debug!(
            "Removed voice finish trigger '{}' from final transcript",
            matched.matched_phrase
        );
        matched.cleaned_text
    } else {
        text.to_string()
    }
}

fn interim_progressive_update(
    committed: &str,
    update_text: &str,
    locked_prefix_chars: usize,
) -> Option<ProgressiveInterimUpdate> {
    let replacement = final_progressive_replacement(committed, update_text)?;

    if locked_prefix_chars > 0 {
        let locked_prefix_end = char_to_byte_index(committed, locked_prefix_chars);
        let locked_prefix = &committed[..locked_prefix_end];
        if !replacement.starts_with(locked_prefix) {
            debug!(
                "Skipping progressive interim replacement that would rewrite locked prefix: locked_prefix_chars={}",
                locked_prefix_chars
            );
            return None;
        }
    }

    let committed_chars = committed.chars().count();
    let (shared_prefix_chars, backspace_chars, _) = progressive_edit_stats(committed, &replacement);

    if committed_chars >= PROGRESSIVE_INTERIM_CHURN_GUARD_MIN_CHARS
        && backspace_chars >= PROGRESSIVE_INTERIM_CHURN_GUARD_MAX_BACKSPACES
        && shared_prefix_chars < PROGRESSIVE_INTERIM_CHURN_GUARD_MIN_PREFIX
    {
        debug!(
            "Skipping unstable progressive interim replacement: committed_chars={}, replacement_chars={}, shared_prefix_chars={}, backspace_chars={}",
            committed_chars,
            replacement.chars().count(),
            shared_prefix_chars,
            backspace_chars
        );
        return None;
    }

    let locked_boundary = last_sentence_boundary_before_or_at(&replacement, shared_prefix_chars);
    let locked_prefix_chars = locked_prefix_chars.max(locked_boundary);

    Some(ProgressiveInterimUpdate {
        text: replacement,
        locked_prefix_chars,
    })
}

fn interim_progressive_replacement(committed: &str, update_text: &str) -> Option<String> {
    interim_progressive_update(committed, update_text, 0).map(|update| update.text)
}

fn insert_progressive_on_main(app: &AppHandle, text: String) -> Result<(), String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let app_for_closure = app.clone();
    app.run_on_main_thread(move || {
        let result = utils::insert_progressive_text(text, app_for_closure);
        let _ = tx.send(result);
    })
    .map_err(|e| format!("Failed to schedule progressive insertion: {e:?}"))?;

    rx.recv()
        .map_err(|e| format!("Failed to receive progressive insertion result: {e}"))?
}

fn replace_progressive_on_main(
    app: &AppHandle,
    previous_text: String,
    replacement_text: String,
    expected_target_window_id: Option<isize>,
) -> Result<(), String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let app_for_closure = app.clone();
    app.run_on_main_thread(move || {
        #[cfg(target_os = "windows")]
        let focus_result = ensure_target_window(expected_target_window_id, foreground_window_id());
        #[cfg(not(target_os = "windows"))]
        let focus_result: Result<(), String> = Ok(());

        let result = focus_result.and_then(|_| {
            utils::replace_progressive_text(
                previous_text,
                replacement_text,
                app_for_closure,
                expected_target_window_id,
            )
        });
        let _ = tx.send(result);
    })
    .map_err(|e| format!("Failed to schedule progressive replacement: {e:?}"))?;

    rx.recv()
        .map_err(|e| format!("Failed to receive progressive replacement result: {e}"))?
}

fn start_progressive_transcription_loop(
    app: AppHandle,
    binding_id: String,
    recording_manager: Arc<AudioRecordingManager>,
    transcription_manager: Arc<TranscriptionManager>,
    diagnostic: Option<DiagnosticSession>,
) {
    let target_window_id = foreground_window_id();
    let session = Arc::new(ProgressiveSession::new(diagnostic, target_window_id));
    PROGRESSIVE_SESSIONS
        .lock()
        .unwrap()
        .insert(binding_id.clone(), Arc::clone(&session));

    std::thread::spawn(move || loop {
        std::thread::sleep(PROGRESSIVE_CHUNK_INTERVAL);

        if session.stopping.load(Ordering::Relaxed) || !recording_manager.is_recording() {
            break;
        }

        if !session.try_begin_inference() {
            break;
        }
        let generation = session.next_generation();
        let result = (|| {
            let drained_samples = recording_manager.drain_recording_chunk(&binding_id);
            let drained_sample_count = drained_samples.as_ref().map_or(0, Vec::len);
            if let Some(samples) = drained_samples {
                session.push_audio(&samples);
                if let Some(diagnostic) = session.diagnostic() {
                    diagnostic.record_json(
                        "progressive_audio_chunk",
                        json!({
                            "samples": samples.len(),
                            "total_audio_samples": session.audio_sample_count(),
                        }),
                    );
                }
            }

            if session.observe_audio_for_leak(drained_sample_count) {
                warn!(
                    "Progressive session watchdog observed five minutes without retained audio; requesting stop"
                );
                if let Some(diagnostic) = session.diagnostic() {
                    diagnostic.record_json(
                        "progressive_leak_watchdog",
                        json!({
                            "status": "stop_requested",
                            "empty_audio_ms": PROGRESSIVE_EMPTY_AUDIO_WATCHDOG_MS,
                            "generation": generation,
                        }),
                    );
                }
                if let Some(coordinator) = app.try_state::<TranscriptionCoordinator>() {
                    coordinator.send_input(&binding_id, "progressive_leak_watchdog", true, false);
                }
                return Ok(());
            }

            if session.stopping.load(Ordering::Relaxed) {
                return Ok(());
            }

            let Some(_new_audio) = session.take_ready_audio() else {
                return Ok(());
            };

            let audio_so_far = session.all_audio();
            if audio_so_far.is_empty() {
                return Ok(());
            }

            let transcription_start = Instant::now();
            match transcription_manager.transcribe_chunk(audio_so_far) {
                Ok(text) => {
                    let transcription_elapsed = transcription_start.elapsed();
                    if session.stopping.load(Ordering::Acquire) {
                        if let Some(diagnostic) = session.diagnostic() {
                            diagnostic.record_json(
                                "progressive_result_discarded",
                                json!({
                                    "generation": generation,
                                    "reason": "stop_requested",
                                    "audio_samples": session.audio_sample_count(),
                                }),
                            );
                        }
                        return Ok(());
                    }
                    debug!(
                        "Progressive interim transcription completed in {:?}: text_chars={}",
                        transcription_elapsed,
                        text.chars().count()
                    );
                    if let Some(diagnostic) = session.diagnostic() {
                        diagnostic.record_json(
                            "progressive_interim_transcript",
                            json!({
                                "elapsed_ms": transcription_elapsed.as_millis(),
                                "generation": generation,
                                "audio_samples": session.audio_sample_count(),
                                "text_chars": text.chars().count(),
                                "text": text.as_str(),
                            }),
                        );
                    }
                    let settings = get_settings(&app);
                    let committed = session.committed_text();
                    let locked_prefix_chars = session.locked_prefix_chars();
                    let voice_finish = if settings.voice_finish_trigger_enabled {
                        detect_voice_finish_trigger(
                            &text,
                            &settings.voice_finish_phrases,
                            &settings.voice_finish_phrase_variants,
                        )
                    } else {
                        None
                    };
                    let candidate_text = voice_finish
                        .as_ref()
                        .map(|matched| matched.cleaned_text.as_str())
                        .unwrap_or(&text);
                    if let (Some(diagnostic), Some(matched)) =
                        (session.diagnostic(), voice_finish.as_ref())
                    {
                        diagnostic.record_json(
                            "voice_finish_match",
                            json!({
                                "matched_phrase": matched.matched_phrase.as_str(),
                                "cleaned_text": matched.cleaned_text.as_str(),
                            }),
                        );
                    }
                    let live_enabled =
                        live_progressive_enabled_for_mode(settings.dictation_stability_mode);
                    let accepted_update = if live_enabled {
                        match settings.dictation_stability_mode {
                            DictationStabilityMode::FastLive => {
                                interim_progressive_replacement(&committed, candidate_text).map(
                                    |text| ProgressiveInterimUpdate {
                                        text,
                                        locked_prefix_chars,
                                    },
                                )
                            }
                            DictationStabilityMode::StableLive => interim_progressive_update(
                                &committed,
                                candidate_text,
                                locked_prefix_chars,
                            ),
                            DictationStabilityMode::FinalOnly => None,
                        }
                    } else {
                        None
                    };
                    let accepted_by_stability = accepted_update.is_some();
                    let update = match accepted_update {
                        Some(accepted) => session.offer_visible_update(generation, accepted),
                        None => session.poll_visible_update(),
                    }
                    .map(|(_, update)| update);

                    if update.is_some() && !session.target_is_foreground() {
                        warn!(
                            "Skipping progressive target edit because the original target window is not foreground"
                        );
                        if let Some(diagnostic) = session.diagnostic() {
                            diagnostic.record_json(
                                "progressive_focus_guard",
                                json!({
                                    "generation": generation,
                                    "status": "skipped",
                                    "reason": "target_window_changed",
                                    "expected_window": session.target_window_id,
                                    "current_window": foreground_window_id(),
                                }),
                            );
                        }
                        emit_progressive_overlay_diagnostics(
                            &app,
                            progressive_overlay_diagnostics(
                                "focus",
                                &committed,
                                None,
                                locked_prefix_chars,
                                transcription_elapsed,
                            ),
                        );
                        if let Some(matched) = voice_finish.as_ref() {
                            info!(
                                "Voice finish trigger matched '{}' while focus guard blocked editing; requesting transcription stop",
                                matched.matched_phrase
                            );
                            if let Some(coordinator) = app.try_state::<TranscriptionCoordinator>() {
                                coordinator.send_input(
                                    &binding_id,
                                    "voice_finish_trigger",
                                    true,
                                    false,
                                );
                            }
                        }
                        return Ok(());
                    }

                    if let Some(update) = update {
                        debug!(
                            "Applying progressive interim replacement: committed_chars={}, replacement_chars={}, locked_prefix_chars={}",
                            committed.chars().count(),
                            update.text.chars().count(),
                            update.locked_prefix_chars
                        );
                        if let Err(e) = replace_progressive_on_main(
                            &app,
                            committed.clone(),
                            update.text.clone(),
                            session.target_window_id,
                        ) {
                            error!("Failed to replace progressive text: {}", e);
                            if let Some(diagnostic) = session.diagnostic() {
                                diagnostic.record_json(
                                    "progressive_edit_error",
                                    json!({
                                        "error": e,
                                        "previous_text": committed.as_str(),
                                        "replacement_text": update.text.as_str(),
                                    }),
                                );
                            }
                        } else {
                            if let Some(diagnostic) = session.diagnostic() {
                                let (shared_prefix_chars, backspaces, insertion_chars) =
                                    progressive_edit_stats(&committed, &update.text);
                                diagnostic.record_json(
                                    "progressive_edit",
                                    json!({
                                        "status": "edit",
                                        "elapsed_ms": transcription_elapsed.as_millis(),
                                        "previous_chars": committed.chars().count(),
                                        "replacement_chars": update.text.chars().count(),
                                        "shared_prefix_chars": shared_prefix_chars,
                                        "backspaces": backspaces,
                                        "insertion_chars": insertion_chars,
                                        "locked_prefix_chars": update.locked_prefix_chars,
                                        "previous_text": committed.as_str(),
                                        "replacement_text": update.text.as_str(),
                                    }),
                                );
                            }
                            let diagnostics = progressive_overlay_diagnostics(
                                "edit",
                                &committed,
                                Some(&update.text),
                                update.locked_prefix_chars,
                                transcription_elapsed,
                            );
                            session.replace_committed_text(update.text);
                            session.replace_locked_prefix_chars(update.locked_prefix_chars);
                            emit_progressive_overlay_diagnostics(&app, diagnostics);
                        }
                    } else if accepted_by_stability {
                        debug!("Accepted progressive hypothesis coalesced by display cadence");
                        if let Some(diagnostic) = session.diagnostic() {
                            diagnostic.record_json(
                                "progressive_coalesced",
                                json!({
                                    "generation": generation,
                                    "reason": "visible_update_throttle",
                                    "visible_interval_ms": PROGRESSIVE_VISIBLE_UPDATE_INTERVAL_MS,
                                    "candidate_text": candidate_text,
                                }),
                            );
                        }
                    } else {
                        debug!("Progressive interim replacement had no stable edit to apply");
                        let status = if voice_finish.is_some() {
                            "finish"
                        } else if text.trim().is_empty() || text.trim() == committed.trim() {
                            "live"
                        } else {
                            "skip"
                        };
                        if let Some(diagnostic) = session.diagnostic() {
                            let (shared_prefix_chars, backspaces, insertion_chars) =
                                progressive_edit_stats(&committed, candidate_text);
                            diagnostic.record_json(
                                "progressive_skip",
                                json!({
                                    "status": status,
                                    "elapsed_ms": transcription_elapsed.as_millis(),
                                    "committed_chars": committed.chars().count(),
                                    "candidate_chars": candidate_text.chars().count(),
                                    "shared_prefix_chars": shared_prefix_chars,
                                    "would_backspace_chars": backspaces,
                                    "would_insert_chars": insertion_chars,
                                    "locked_prefix_chars": locked_prefix_chars,
                                    "committed_text": committed.as_str(),
                                    "candidate_text": candidate_text,
                                }),
                            );
                        }
                        emit_progressive_overlay_diagnostics(
                            &app,
                            progressive_overlay_diagnostics(
                                status,
                                &committed,
                                None,
                                locked_prefix_chars,
                                transcription_elapsed,
                            ),
                        );
                    }

                    if let Some(matched) = voice_finish {
                        info!(
                            "Voice finish trigger matched '{}'; requesting transcription stop",
                            matched.matched_phrase
                        );
                        if let Some(coordinator) = app.try_state::<TranscriptionCoordinator>() {
                            coordinator.send_input(
                                &binding_id,
                                "voice_finish_trigger",
                                true,
                                false,
                            );
                        } else {
                            warn!("Voice finish trigger could not stop transcription: coordinator unavailable");
                        }
                    }
                }
                Err(e) => {
                    error!("Progressive transcription chunk failed: {}", e);
                    if let Some(diagnostic) = session.diagnostic() {
                        diagnostic.record_json(
                            "progressive_transcription_error",
                            json!({ "error": e.to_string() }),
                        );
                    }
                }
            }

            Ok::<(), ()>(())
        })();

        if result.is_err() {
            error!("Progressive transcription loop failed unexpectedly");
        }
        session.set_in_flight(false);
    });
}

impl ShortcutAction for TranscribeAction {
    fn start(&self, app: &AppHandle, binding_id: &str, _shortcut_str: &str) {
        let start_time = Instant::now();
        debug!("TranscribeAction::start called for binding: {}", binding_id);

        // Load model in the background
        let tm = app.state::<Arc<TranscriptionManager>>();
        tm.initiate_model_load();

        let binding_id = binding_id.to_string();
        change_tray_icon(app, TrayIconState::Recording);
        show_recording_overlay(app);

        let rm = app.state::<Arc<AudioRecordingManager>>();

        let rm_for_vad = Arc::clone(&rm);
        std::thread::spawn(move || {
            if let Err(e) = rm_for_vad.preload_vad() {
                debug!("VAD pre-load failed: {}", e);
            }
        });

        // Get the microphone mode to determine audio feedback timing
        let settings = get_settings(app);
        let is_always_on = settings.always_on_microphone;
        debug!("Microphone mode - always_on: {}", is_always_on);
        let diagnostic = DiagnosticSession::start(app, &binding_id, self.post_process, &settings);
        if let Some(diagnostic) = diagnostic.clone() {
            DIAGNOSTIC_SESSIONS
                .lock()
                .unwrap()
                .insert(binding_id.clone(), diagnostic);
        }

        let mut recording_error: Option<String> = None;
        if is_always_on {
            // Always-on mode: Play audio feedback immediately, then apply mute after sound finishes
            debug!("Always-on mode: Playing audio feedback immediately");
            let rm_clone = Arc::clone(&rm);
            let app_clone = app.clone();
            // The blocking helper exits immediately if audio feedback is disabled,
            // so we can always reuse this thread to ensure mute happens right after playback.
            std::thread::spawn(move || {
                play_feedback_sound_blocking(&app_clone, SoundType::Start);
                rm_clone.apply_mute();
            });

            if let Err(e) = rm.try_start_recording(&binding_id) {
                debug!("Recording failed: {}", e);
                if let Some(diagnostic) = diagnostic.as_ref() {
                    diagnostic.record_json(
                        "recording_error",
                        json!({
                            "mode": "always_on",
                            "error": e.as_str(),
                        }),
                    );
                }
                recording_error = Some(e);
            } else if let Some(diagnostic) = diagnostic.as_ref() {
                diagnostic.record_json(
                    "recording_start",
                    json!({
                        "mode": "always_on",
                        "start_elapsed_ms": start_time.elapsed().as_millis(),
                    }),
                );
            }
        } else {
            // On-demand mode: Start recording first, then play audio feedback, then apply mute
            // This allows the microphone to be activated before playing the sound
            debug!("On-demand mode: Starting recording first, then audio feedback");
            let recording_start_time = Instant::now();
            match rm.try_start_recording(&binding_id) {
                Ok(()) => {
                    debug!("Recording started in {:?}", recording_start_time.elapsed());
                    if let Some(diagnostic) = diagnostic.as_ref() {
                        diagnostic.record_json(
                            "recording_start",
                            json!({
                                "mode": "on_demand",
                                "recording_start_elapsed_ms": recording_start_time.elapsed().as_millis(),
                                "action_start_elapsed_ms": start_time.elapsed().as_millis(),
                            }),
                        );
                    }
                    // Small delay to ensure microphone stream is active
                    let app_clone = app.clone();
                    let rm_clone = Arc::clone(&rm);
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                        debug!("Handling delayed audio feedback/mute sequence");
                        // Helper handles disabled audio feedback by returning early, so we reuse it
                        // to keep mute sequencing consistent in every mode.
                        play_feedback_sound_blocking(&app_clone, SoundType::Start);
                        rm_clone.apply_mute();
                    });
                }
                Err(e) => {
                    debug!("Failed to start recording: {}", e);
                    if let Some(diagnostic) = diagnostic.as_ref() {
                        diagnostic.record_json(
                            "recording_error",
                            json!({
                                "mode": "on_demand",
                                "error": e.as_str(),
                            }),
                        );
                    }
                    recording_error = Some(e);
                }
            }
        }

        if recording_error.is_none() {
            // Dynamically register the cancel shortcut in a separate task to avoid deadlock
            shortcut::register_cancel_shortcut(app);
            if !self.post_process
                && (live_progressive_enabled_for_mode(settings.dictation_stability_mode)
                    || settings.voice_finish_trigger_enabled)
            {
                start_progressive_transcription_loop(
                    app.clone(),
                    binding_id.clone(),
                    Arc::clone(&rm),
                    Arc::clone(&tm),
                    diagnostic.clone(),
                );
            }
        } else {
            // Starting failed (for example due to blocked microphone permissions).
            // Revert UI state so we don't stay stuck in the recording overlay.
            utils::hide_recording_overlay(app);
            change_tray_icon(app, TrayIconState::Idle);
            DIAGNOSTIC_SESSIONS.lock().unwrap().remove(&binding_id);
            if let Some(err) = recording_error {
                let event = if is_microphone_access_denied(&err) {
                    RecordingErrorEvent {
                        error_type: "permission_denied".to_string(),
                        detail: Some(err),
                    }
                } else if is_no_input_device_error(&err) {
                    RecordingErrorEvent {
                        error_type: "no_input_device".to_string(),
                        detail: Some(err),
                    }
                } else {
                    RecordingErrorEvent {
                        error_type: "other".to_string(),
                        detail: Some(err),
                    }
                };
                let _ = app.emit("recording-error", event);
            }
        }

        debug!(
            "TranscribeAction::start completed in {:?}",
            start_time.elapsed()
        );
    }

    fn stop(&self, app: &AppHandle, binding_id: &str, _shortcut_str: &str) {
        // Unregister the cancel shortcut when transcription stops
        shortcut::unregister_cancel_shortcut(app);

        let stop_time = Instant::now();
        debug!("TranscribeAction::stop called for binding: {}", binding_id);

        let ah = app.clone();
        let rm = Arc::clone(&app.state::<Arc<AudioRecordingManager>>());
        let tm = Arc::clone(&app.state::<Arc<TranscriptionManager>>());
        let hm = Arc::clone(&app.state::<Arc<HistoryManager>>());

        change_tray_icon(app, TrayIconState::Transcribing);
        show_transcribing_overlay(app);

        // Unmute before playing audio feedback so the stop sound is audible
        rm.remove_mute();

        // Play audio feedback for recording stop
        play_feedback_sound(app, SoundType::Stop);

        let binding_id = binding_id.to_string(); // Clone binding_id for the async task
        let diagnostic = DIAGNOSTIC_SESSIONS.lock().unwrap().remove(&binding_id);
        let post_process = self.post_process;

        tauri::async_runtime::spawn(async move {
            let _guard = FinishGuard(ah.clone());
            let binding_id = binding_id.clone(); // Clone for the inner async task
            debug!(
                "Starting async transcription task for binding: {}",
                binding_id
            );

            if !post_process
                && PROGRESSIVE_SESSIONS
                    .lock()
                    .unwrap()
                    .contains_key(&binding_id)
            {
                let session = PROGRESSIVE_SESSIONS.lock().unwrap().remove(&binding_id);
                if let Some(session) = &session {
                    let interim_wait_started = Instant::now();
                    if let Some(diagnostic) = session.diagnostic() {
                        diagnostic.record_json(
                            "progressive_stop_requested",
                            json!({
                                "generation": session.generation.load(Ordering::Acquire),
                                "in_flight": *session.in_flight.lock().unwrap(),
                                "audio_samples": session.audio_sample_count(),
                            }),
                        );
                    }
                    session.begin_stopping_and_wait();
                    if let Some(diagnostic) = session.diagnostic() {
                        diagnostic.record_json(
                            "progressive_stop_interim_wait",
                            json!({
                                "elapsed_ms": interim_wait_started.elapsed().as_millis(),
                            }),
                        );
                    }
                }

                let stop_recording_time = Instant::now();
                if let Some(samples) = rm.stop_recording(&binding_id) {
                    let stop_recording_elapsed = stop_recording_time.elapsed();
                    debug!(
                        "Progressive recording stopped and tail samples retrieved in {:?}, sample count: {}",
                        stop_recording_elapsed,
                        samples.len()
                    );
                    if let Some(diagnostic) = diagnostic.as_ref() {
                        diagnostic.record_json(
                            "recording_stop",
                            json!({
                                "mode": "progressive",
                                "stop_recording_elapsed_ms": stop_recording_elapsed.as_millis(),
                                "tail_samples": samples.len(),
                            }),
                        );
                    }

                    let mut final_audio = session
                        .as_ref()
                        .map(|session| session.take_pending_audio())
                        .unwrap_or_default();
                    let pending_samples = final_audio.len();
                    final_audio.extend_from_slice(&samples);

                    if let Some(session) = &session {
                        session
                            .all_audio
                            .lock()
                            .unwrap()
                            .extend_from_slice(&samples);
                    }

                    if let Some(session) = &session {
                        let progressive_diagnostic =
                            session.diagnostic().or_else(|| diagnostic.clone());
                        let full_audio = session.all_audio();
                        if !full_audio.is_empty() {
                            let full_audio_samples = full_audio.len();
                            let full_transcription_time = Instant::now();
                            match tm.transcribe(full_audio) {
                                Ok(mut full_text) => {
                                    let full_transcription_elapsed =
                                        full_transcription_time.elapsed();
                                    if let Some(diagnostic) = progressive_diagnostic.as_ref() {
                                        diagnostic.record_json(
                                            "final_full_transcript",
                                            json!({
                                                "mode": "progressive",
                                                "elapsed_ms": full_transcription_elapsed.as_millis(),
                                                "audio_samples": full_audio_samples,
                                                "text_chars": full_text.chars().count(),
                                                "text": full_text.as_str(),
                                            }),
                                        );
                                    }
                                    let settings = get_settings(&ah);
                                    if settings.voice_finish_trigger_enabled {
                                        if let Some(matched) = detect_voice_finish_trigger(
                                            &full_text,
                                            &settings.voice_finish_phrases,
                                            &settings.voice_finish_phrase_variants,
                                        ) {
                                            debug!(
                                                "Removed voice finish trigger '{}' from final progressive transcript",
                                                matched.matched_phrase
                                            );
                                            if let Some(diagnostic) =
                                                progressive_diagnostic.as_ref()
                                            {
                                                diagnostic.record_json(
                                                    "final_voice_finish_cleanup",
                                                    json!({
                                                        "phase": "full_reconciliation",
                                                        "matched_phrase": matched.matched_phrase.as_str(),
                                                        "before_text": full_text.as_str(),
                                                        "cleaned_text": matched.cleaned_text.as_str(),
                                                    }),
                                                );
                                            }
                                            full_text = matched.cleaned_text;
                                        }
                                    }
                                    let committed = session.committed_text();
                                    if let Some(replacement) =
                                        final_progressive_replacement(&committed, &full_text)
                                    {
                                        let (shared_prefix_chars, backspaces, insertion_chars) =
                                            progressive_edit_stats(&committed, &replacement);
                                        if let Some(diagnostic) = progressive_diagnostic.as_ref() {
                                            diagnostic.record_json(
                                                "final_reconciliation_edit",
                                                json!({
                                                    "previous_chars": committed.chars().count(),
                                                    "replacement_chars": replacement.chars().count(),
                                                    "shared_prefix_chars": shared_prefix_chars,
                                                    "backspaces": backspaces,
                                                    "insertion_chars": insertion_chars,
                                                    "previous_text": committed.as_str(),
                                                    "replacement_text": replacement.as_str(),
                                                }),
                                            );
                                        }
                                        if !session.target_is_foreground() {
                                            warn!(
                                                "Skipping final progressive reconciliation because the original target window is not foreground"
                                            );
                                            if let Some(diagnostic) =
                                                progressive_diagnostic.as_ref()
                                            {
                                                diagnostic.record_json(
                                                    "final_reconciliation_focus_guard",
                                                    json!({
                                                        "status": "skipped",
                                                        "reason": "target_window_changed",
                                                        "expected_window": session.target_window_id,
                                                        "current_window": foreground_window_id(),
                                                    }),
                                                );
                                            }
                                        } else if let Err(e) = replace_progressive_on_main(
                                            &ah,
                                            committed.clone(),
                                            replacement.clone(),
                                            session.target_window_id,
                                        ) {
                                            error!(
                                                "Failed to replace interim progressive text: {}",
                                                e
                                            );
                                            if let Some(diagnostic) =
                                                progressive_diagnostic.as_ref()
                                            {
                                                diagnostic.record_json(
                                                    "final_reconciliation_error",
                                                    json!({
                                                        "error": e.to_string(),
                                                        "previous_text": committed.as_str(),
                                                        "replacement_text": replacement.as_str(),
                                                    }),
                                                );
                                            }
                                        } else {
                                            session.replace_committed_text(replacement);
                                        }
                                    } else if let Some(diagnostic) = progressive_diagnostic.as_ref()
                                    {
                                        diagnostic.record_json(
                                            "final_reconciliation_skip",
                                            json!({
                                                "reason": "no_replacement_needed",
                                                "committed_chars": committed.chars().count(),
                                                "final_chars": full_text.chars().count(),
                                                "committed_text": committed.as_str(),
                                                "final_text": full_text.as_str(),
                                            }),
                                        );
                                    }
                                }
                                Err(e) => {
                                    error!(
                                        "Full progressive transcription reconciliation failed: {}",
                                        e
                                    );
                                    if let Some(diagnostic) = progressive_diagnostic.as_ref() {
                                        diagnostic.record_json(
                                            "final_full_transcription_error",
                                            json!({
                                                "mode": "progressive",
                                                "audio_samples": full_audio_samples,
                                                "error": e.to_string(),
                                            }),
                                        );
                                    }
                                }
                            }
                        } else if !final_audio.is_empty() {
                            let fallback_audio_samples = final_audio.len();
                            let fallback_transcription_time = Instant::now();
                            match tm.transcribe(final_audio) {
                                Ok(mut text) => {
                                    let fallback_transcription_elapsed =
                                        fallback_transcription_time.elapsed();
                                    if let Some(diagnostic) = progressive_diagnostic.as_ref() {
                                        diagnostic.record_json(
                                            "final_fallback_transcript",
                                            json!({
                                                "mode": "progressive",
                                                "elapsed_ms": fallback_transcription_elapsed.as_millis(),
                                                "audio_samples": fallback_audio_samples,
                                                "text_chars": text.chars().count(),
                                                "text": text.as_str(),
                                            }),
                                        );
                                    }
                                    let settings = get_settings(&ah);
                                    if settings.voice_finish_trigger_enabled {
                                        if let Some(matched) = detect_voice_finish_trigger(
                                            &text,
                                            &settings.voice_finish_phrases,
                                            &settings.voice_finish_phrase_variants,
                                        ) {
                                            debug!(
                                                "Removed voice finish trigger '{}' from final progressive fallback transcript",
                                                matched.matched_phrase
                                            );
                                            if let Some(diagnostic) =
                                                progressive_diagnostic.as_ref()
                                            {
                                                diagnostic.record_json(
                                                    "final_voice_finish_cleanup",
                                                    json!({
                                                        "phase": "fallback_final",
                                                        "matched_phrase": matched.matched_phrase.as_str(),
                                                        "before_text": text.as_str(),
                                                        "cleaned_text": matched.cleaned_text.as_str(),
                                                    }),
                                                );
                                            }
                                            text = matched.cleaned_text;
                                        }
                                    }
                                    if let Err(e) = insert_progressive_on_main(&ah, text.clone()) {
                                        error!(
                                            "Failed to insert final progressive fallback text: {}",
                                            e
                                        );
                                        if let Some(diagnostic) = progressive_diagnostic.as_ref() {
                                            diagnostic.record_json(
                                                "final_fallback_insert_error",
                                                json!({
                                                    "error": e.to_string(),
                                                    "text": text.as_str(),
                                                }),
                                            );
                                        }
                                    } else {
                                        if let Some(diagnostic) = progressive_diagnostic.as_ref() {
                                            diagnostic.record_json(
                                                "final_fallback_insert",
                                                json!({
                                                    "text_chars": text.chars().count(),
                                                    "text": text.as_str(),
                                                }),
                                            );
                                        }
                                        session.replace_committed_text(text);
                                    }
                                }
                                Err(e) => {
                                    error!("Final progressive transcription failed: {}", e);
                                    if let Some(diagnostic) = progressive_diagnostic.as_ref() {
                                        diagnostic.record_json(
                                            "final_fallback_transcription_error",
                                            json!({
                                                "audio_samples": fallback_audio_samples,
                                                "error": e.to_string(),
                                            }),
                                        );
                                    }
                                }
                            }
                        } else if let Some(diagnostic) = progressive_diagnostic.as_ref() {
                            diagnostic.record_json(
                                "final_audio_empty",
                                json!({
                                    "pending_samples": pending_samples,
                                    "tail_samples": samples.len(),
                                }),
                            );
                        }

                        let final_text = session.committed_text();
                        if !final_text.trim().is_empty() {
                            let samples_for_history = session.all_audio.lock().unwrap().clone();
                            let hm_clone = Arc::clone(&hm);
                            let diagnostic_for_history =
                                session.diagnostic().or_else(|| diagnostic.clone());
                            let final_text_chars = final_text.chars().count();
                            tauri::async_runtime::spawn(async move {
                                match hm_clone
                                    .save_transcription(
                                        samples_for_history,
                                        final_text,
                                        false,
                                        None,
                                        None,
                                    )
                                    .await
                                {
                                    Ok(file_name) => {
                                        if let Some(diagnostic) = diagnostic_for_history.as_ref() {
                                            diagnostic.record_json(
                                                "history_saved",
                                                json!({
                                                    "mode": "progressive",
                                                    "wav_file": file_name,
                                                    "text_chars": final_text_chars,
                                                }),
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        error!(
                                            "Failed to save progressive transcription to history: {}",
                                            e
                                        );
                                        if let Some(diagnostic) = diagnostic_for_history.as_ref() {
                                            diagnostic.record_json(
                                                "history_error",
                                                json!({
                                                    "mode": "progressive",
                                                    "error": e.to_string(),
                                                }),
                                            );
                                        }
                                    }
                                }
                            });
                        } else if let Some(diagnostic) = progressive_diagnostic.as_ref() {
                            diagnostic.record_json(
                                "history_skipped",
                                json!({
                                    "mode": "progressive",
                                    "reason": "empty_final_text",
                                }),
                            );
                        }
                    }
                } else {
                    debug!("No samples retrieved from progressive recording stop");
                    if let Some(diagnostic) = diagnostic.as_ref() {
                        diagnostic.record_json(
                            "recording_stop",
                            json!({
                                "mode": "progressive",
                                "status": "no_samples",
                                "stop_recording_elapsed_ms": stop_recording_time.elapsed().as_millis(),
                            }),
                        );
                    }
                }

                utils::hide_recording_overlay(&ah);
                change_tray_icon(&ah, TrayIconState::Idle);
                return;
            }

            let stop_recording_time = Instant::now();
            if let Some(samples) = rm.stop_recording(&binding_id) {
                let stop_recording_elapsed = stop_recording_time.elapsed();
                let sample_count = samples.len();
                let diagnostic_mode = if post_process {
                    "post_process"
                } else {
                    "final_only"
                };
                debug!(
                    "Recording stopped and samples retrieved in {:?}, sample count: {}",
                    stop_recording_elapsed, sample_count
                );
                if let Some(diagnostic) = diagnostic.as_ref() {
                    diagnostic.record_json(
                        "recording_stop",
                        json!({
                            "mode": diagnostic_mode,
                            "stop_recording_elapsed_ms": stop_recording_elapsed.as_millis(),
                            "samples": sample_count,
                        }),
                    );
                }

                let transcription_time = Instant::now();
                let samples_clone = samples.clone(); // Clone for history saving
                match tm.transcribe(samples) {
                    Ok(transcription) => {
                        let transcription_elapsed = transcription_time.elapsed();
                        debug!(
                            "Transcription completed in {:?}: '{}'",
                            transcription_elapsed, transcription
                        );
                        if let Some(diagnostic) = diagnostic.as_ref() {
                            diagnostic.record_json(
                                "final_transcript",
                                json!({
                                    "mode": diagnostic_mode,
                                    "elapsed_ms": transcription_elapsed.as_millis(),
                                    "audio_samples": sample_count,
                                    "text_chars": transcription.chars().count(),
                                    "text": transcription.as_str(),
                                }),
                            );
                        }
                        if !transcription.is_empty() {
                            let settings = get_settings(&ah);
                            let mut final_text = transcription.clone();
                            let mut post_processed_text: Option<String> = None;
                            let mut post_process_prompt: Option<String> = None;

                            // First, check if Chinese variant conversion is needed
                            if let Some(converted_text) =
                                maybe_convert_chinese_variant(&settings, &transcription).await
                            {
                                final_text = converted_text;
                            }

                            final_text = cleanup_final_text_for_paste(&final_text, &settings);

                            // Then apply LLM post-processing if this is the post-process hotkey
                            // Uses final_text which may already have Chinese conversion applied
                            if post_process {
                                show_processing_overlay(&ah);
                            }
                            let post_process_time = Instant::now();
                            let processed = if post_process {
                                post_process_transcription(&settings, &final_text).await
                            } else {
                                None
                            };
                            if post_process {
                                if let Some(diagnostic) = diagnostic.as_ref() {
                                    diagnostic.record_json(
                                        "post_process_result",
                                        json!({
                                            "elapsed_ms": post_process_time.elapsed().as_millis(),
                                            "status": if processed.is_some() { "updated" } else { "no_output" },
                                            "input_text": final_text.as_str(),
                                            "processed_text": processed.as_deref(),
                                        }),
                                    );
                                }
                            }
                            if let Some(processed_text) = processed {
                                post_processed_text = Some(processed_text.clone());
                                final_text = processed_text;

                                // Get the prompt that was used
                                if let Some(prompt_id) = &settings.post_process_selected_prompt_id {
                                    if let Some(prompt) = settings
                                        .post_process_prompts
                                        .iter()
                                        .find(|p| &p.id == prompt_id)
                                    {
                                        post_process_prompt = Some(prompt.prompt.clone());
                                    }
                                }
                            } else if final_text != transcription {
                                // Chinese conversion was applied but no LLM post-processing
                                post_processed_text = Some(final_text.clone());
                            }
                            if let Some(diagnostic) = diagnostic.as_ref() {
                                diagnostic.record_json(
                                    "final_text_ready",
                                    json!({
                                        "mode": diagnostic_mode,
                                        "raw_text_chars": transcription.chars().count(),
                                        "final_text_chars": final_text.chars().count(),
                                        "post_processed": post_processed_text.is_some(),
                                        "raw_text": transcription.as_str(),
                                        "final_text": final_text.as_str(),
                                        "post_processed_text": post_processed_text.as_deref(),
                                    }),
                                );
                            }

                            // Save to history with post-processed text and prompt
                            let hm_clone = Arc::clone(&hm);
                            let transcription_for_history = transcription.clone();
                            let diagnostic_for_history = diagnostic.clone();
                            let history_raw_text_chars = transcription_for_history.chars().count();
                            let history_post_processed = post_processed_text.is_some();
                            tauri::async_runtime::spawn(async move {
                                match hm_clone
                                    .save_transcription(
                                        samples_clone,
                                        transcription_for_history,
                                        post_process,
                                        post_processed_text,
                                        post_process_prompt,
                                    )
                                    .await
                                {
                                    Ok(file_name) => {
                                        if let Some(diagnostic) = diagnostic_for_history.as_ref() {
                                            diagnostic.record_json(
                                                "history_saved",
                                                json!({
                                                    "mode": diagnostic_mode,
                                                    "wav_file": file_name,
                                                    "raw_text_chars": history_raw_text_chars,
                                                    "post_processed": history_post_processed,
                                                }),
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        error!("Failed to save transcription to history: {}", e);
                                        if let Some(diagnostic) = diagnostic_for_history.as_ref() {
                                            diagnostic.record_json(
                                                "history_error",
                                                json!({
                                                    "mode": diagnostic_mode,
                                                    "error": e.to_string(),
                                                }),
                                            );
                                        }
                                    }
                                }
                            });

                            // Paste the final text (either processed or original)
                            let ah_clone = ah.clone();
                            let diagnostic_for_paste = diagnostic.clone();
                            let final_text_chars = final_text.chars().count();
                            let paste_time = Instant::now();
                            ah.run_on_main_thread(move || {
                                match utils::paste(final_text, ah_clone.clone()) {
                                    Ok(()) => {
                                        let paste_elapsed = paste_time.elapsed();
                                        debug!("Text pasted successfully in {:?}", paste_elapsed);
                                        if let Some(diagnostic) = diagnostic_for_paste.as_ref() {
                                            diagnostic.record_json(
                                                "paste_result",
                                                json!({
                                                    "status": "ok",
                                                    "mode": diagnostic_mode,
                                                    "elapsed_ms": paste_elapsed.as_millis(),
                                                    "text_chars": final_text_chars,
                                                }),
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        error!("Failed to paste transcription: {}", e);
                                        if let Some(diagnostic) = diagnostic_for_paste.as_ref() {
                                            diagnostic.record_json(
                                                "paste_result",
                                                json!({
                                                    "status": "error",
                                                    "mode": diagnostic_mode,
                                                    "elapsed_ms": paste_time.elapsed().as_millis(),
                                                    "text_chars": final_text_chars,
                                                    "error": e.to_string(),
                                                }),
                                            );
                                        }
                                    }
                                }
                                // Hide the overlay after transcription is complete
                                utils::hide_recording_overlay(&ah_clone);
                                change_tray_icon(&ah_clone, TrayIconState::Idle);
                            })
                            .unwrap_or_else(|e| {
                                error!("Failed to run paste on main thread: {:?}", e);
                                if let Some(diagnostic) = diagnostic.as_ref() {
                                    diagnostic.record_json(
                                        "paste_result",
                                        json!({
                                            "status": "main_thread_error",
                                            "mode": diagnostic_mode,
                                            "elapsed_ms": paste_time.elapsed().as_millis(),
                                            "text_chars": final_text_chars,
                                            "error": format!("{:?}", e),
                                        }),
                                    );
                                }
                                utils::hide_recording_overlay(&ah);
                                change_tray_icon(&ah, TrayIconState::Idle);
                            });
                        } else {
                            if let Some(diagnostic) = diagnostic.as_ref() {
                                diagnostic.record_json(
                                    "final_text_empty",
                                    json!({
                                        "mode": diagnostic_mode,
                                        "audio_samples": sample_count,
                                    }),
                                );
                            }
                            utils::hide_recording_overlay(&ah);
                            change_tray_icon(&ah, TrayIconState::Idle);
                        }
                    }
                    Err(err) => {
                        debug!("Global Shortcut Transcription error: {}", err);
                        if let Some(diagnostic) = diagnostic.as_ref() {
                            diagnostic.record_json(
                                "final_transcription_error",
                                json!({
                                    "mode": diagnostic_mode,
                                    "audio_samples": sample_count,
                                    "elapsed_ms": transcription_time.elapsed().as_millis(),
                                    "error": err.to_string(),
                                }),
                            );
                        }
                        utils::hide_recording_overlay(&ah);
                        change_tray_icon(&ah, TrayIconState::Idle);
                    }
                }
            } else {
                debug!("No samples retrieved from recording stop");
                if let Some(diagnostic) = diagnostic.as_ref() {
                    diagnostic.record_json(
                        "recording_stop",
                        json!({
                            "mode": if post_process { "post_process" } else { "final_only" },
                            "status": "no_samples",
                            "stop_recording_elapsed_ms": stop_recording_time.elapsed().as_millis(),
                        }),
                    );
                }
                utils::hide_recording_overlay(&ah);
                change_tray_icon(&ah, TrayIconState::Idle);
            }
        });

        debug!(
            "TranscribeAction::stop completed in {:?}",
            stop_time.elapsed()
        );
    }
}

// Cancel Action
struct CancelAction;

impl ShortcutAction for CancelAction {
    fn start(&self, app: &AppHandle, _binding_id: &str, _shortcut_str: &str) {
        utils::cancel_current_operation(app);
    }

    fn stop(&self, _app: &AppHandle, _binding_id: &str, _shortcut_str: &str) {
        // Nothing to do on stop for cancel
    }
}

// Test Action
struct TestAction;

impl ShortcutAction for TestAction {
    fn start(&self, app: &AppHandle, binding_id: &str, shortcut_str: &str) {
        log::info!(
            "Shortcut ID '{}': Started - {} (App: {})", // Changed "Pressed" to "Started" for consistency
            binding_id,
            shortcut_str,
            app.package_info().name
        );
    }

    fn stop(&self, app: &AppHandle, binding_id: &str, shortcut_str: &str) {
        log::info!(
            "Shortcut ID '{}': Stopped - {} (App: {})", // Changed "Released" to "Stopped" for consistency
            binding_id,
            shortcut_str,
            app.package_info().name
        );
    }
}

// Static Action Map
pub static ACTION_MAP: Lazy<HashMap<String, Arc<dyn ShortcutAction>>> = Lazy::new(|| {
    let mut map = HashMap::new();
    map.insert(
        "transcribe".to_string(),
        Arc::new(TranscribeAction {
            post_process: false,
        }) as Arc<dyn ShortcutAction>,
    );
    map.insert(
        "transcribe_with_post_process".to_string(),
        Arc::new(TranscribeAction { post_process: true }) as Arc<dyn ShortcutAction>,
    );
    map.insert(
        "cancel".to_string(),
        Arc::new(CancelAction) as Arc<dyn ShortcutAction>,
    );
    map.insert(
        "test".to_string(),
        Arc::new(TestAction) as Arc<dyn ShortcutAction>,
    );
    map
});

#[cfg(test)]
mod progressive_tests {
    use super::{
        char_to_byte_index, cleanup_final_text_for_paste, detect_voice_finish_trigger,
        final_progressive_replacement, interim_progressive_replacement, interim_progressive_update,
        live_progressive_enabled_for_mode, ProgressiveDisplayGate, ProgressiveLeakWatchdog,
        ProgressiveSession,
    };
    use crate::settings::{get_default_settings, DictationStabilityMode};

    #[test]
    fn full_utterance_transcript_replaces_interim_progressive_text() {
        assert_eq!(
            final_progressive_replacement(
                "I am doing a jewel test. I'm recording this first test with the first.",
                "I'm doing a dual test. So, on the one hand, I'm recording this first test with Whisper."
            ),
            Some(
                "I'm doing a dual test. So, on the one hand, I'm recording this first test with Whisper."
                    .to_string()
            )
        );
    }

    #[test]
    fn empty_full_utterance_transcript_does_not_replace_interim_text() {
        assert_eq!(
            final_progressive_replacement("some interim text", "   "),
            None
        );
    }

    #[test]
    fn interim_progressive_update_replaces_previous_chunk_guess() {
        assert_eq!(
            interim_progressive_replacement(
                "The progressive mode was treating two seconds.",
                "The progressive mode was treating two-second chunk transcripts as final text."
            ),
            Some(
                "The progressive mode was treating two-second chunk transcripts as final text."
                    .to_string()
            )
        );
    }

    #[test]
    fn interim_progressive_update_skips_large_early_rewrite() {
        let committed = "The dictation feature appears to be working reasonably well, but I noticed that it now does the type ahead, and then it goes sometimes back and deletes the text.";
        let update = "I noticed something different in Perplexity where the prompt keeps rewriting from the beginning while I am still speaking.";

        assert_eq!(interim_progressive_replacement(committed, update), None);
    }

    #[test]
    fn interim_progressive_update_locks_sentence_after_it_survives_next_update() {
        let committed = "This first sentence is stable. The current";
        let update = "This first sentence is stable. The current sentence is still moving";

        let result = interim_progressive_update(committed, update, 0).expect("stable update");

        assert_eq!(result.text, update);
        assert_eq!(
            &result.text[..char_to_byte_index(&result.text, result.locked_prefix_chars)],
            "This first sentence is stable. "
        );
    }

    #[test]
    fn interim_progressive_update_does_not_rewrite_locked_sentence() {
        let committed = "This first sentence is stable. The current sentence is still moving";
        let locked_prefix_chars = "This first sentence is stable. ".chars().count();
        let update = "This first sentence was changed. The current sentence is still moving";

        assert_eq!(
            interim_progressive_update(committed, update, locked_prefix_chars),
            None
        );
    }

    #[test]
    fn display_gate_applies_first_accepted_hypothesis_immediately() {
        let mut gate = ProgressiveDisplayGate::default();

        assert_eq!(
            gate.offer(1, 4_100, "first accepted".to_string()),
            Some((1, "first accepted".to_string()))
        );
    }

    #[test]
    fn display_gate_coalesces_only_to_latest_accepted_hypothesis() {
        let mut gate = ProgressiveDisplayGate::default();
        let _ = gate.offer(1, 4_100, "first".to_string());

        assert_eq!(gate.offer(2, 5_000, "second".to_string()), None);
        assert_eq!(gate.offer(3, 6_000, "third".to_string()), None);
        assert_eq!(gate.poll(7_300), Some((3, "third".to_string())));
    }

    #[test]
    fn display_gate_rejects_stale_generations() {
        let mut gate = ProgressiveDisplayGate::default();
        let _ = gate.offer(4, 4_100, "current".to_string());

        assert_eq!(gate.offer(3, 8_000, "stale".to_string()), None);
        assert_eq!(gate.poll(8_000), None);
    }

    #[test]
    fn stopping_display_gate_discards_pending_hypothesis() {
        let mut gate = ProgressiveDisplayGate::default();
        let _ = gate.offer(1, 4_100, "first".to_string());
        let _ = gate.offer(2, 5_000, "pending".to_string());

        gate.stop();

        assert_eq!(gate.poll(8_000), None);
        assert_eq!(gate.offer(3, 8_000, "late".to_string()), None);
    }

    #[test]
    fn leak_watchdog_requests_stop_after_five_minutes_without_audio() {
        let mut watchdog = ProgressiveLeakWatchdog::default();
        assert!(!watchdog.observe(1_000, 16_000));
        assert!(!watchdog.observe(300_999, 0));
        assert!(watchdog.observe(301_000, 0));
    }

    #[test]
    fn leak_watchdog_resets_when_audio_resumes() {
        let mut watchdog = ProgressiveLeakWatchdog::default();
        let _ = watchdog.observe(1_000, 16_000);
        let _ = watchdog.observe(250_000, 0);
        assert!(!watchdog.observe(250_100, 8_000));
        assert!(!watchdog.observe(500_000, 0));
    }

    #[test]
    fn leak_watchdog_fires_only_once() {
        let mut watchdog = ProgressiveLeakWatchdog::default();
        let _ = watchdog.observe(1_000, 16_000);
        assert!(watchdog.observe(301_000, 0));
        assert!(!watchdog.observe(400_000, 0));
    }

    #[test]
    fn leak_watchdog_stops_session_that_never_receives_audio() {
        let mut watchdog = ProgressiveLeakWatchdog::default();
        assert!(!watchdog.observe(299_999, 0));
        assert!(watchdog.observe(300_000, 0));
    }

    #[test]
    fn stop_handshake_prevents_new_interim_inference() {
        let session = ProgressiveSession::new(None, Some(42));

        session.begin_stopping_and_wait();

        assert!(!session.try_begin_inference());
    }

    #[test]
    fn voice_finish_trigger_matches_tail_and_removes_phrase() {
        let result = detect_voice_finish_trigger(
            "Please send this after the final pass. end dictation",
            &["end dictation".to_string()],
            &[],
        )
        .expect("tail phrase should trigger");

        assert_eq!(
            result.cleaned_text,
            "Please send this after the final pass."
        );
        assert_eq!(result.matched_phrase, "end dictation");
    }

    #[test]
    fn voice_finish_trigger_removes_multiple_tail_phrases() {
        let result = detect_voice_finish_trigger(
            "Please check the debug . Send it.Finish dictation.",
            &[
                "finish dictation".to_string(),
                "end dictation".to_string(),
                "send it".to_string(),
            ],
            &[],
        )
        .expect("adjacent tail phrases should trigger");

        assert_eq!(result.cleaned_text, "Please check the debug.");
        assert_eq!(result.matched_phrase, "send it + finish dictation");
    }

    #[test]
    fn voice_finish_trigger_removes_repeated_tail_phrase() {
        let result = detect_voice_finish_trigger(
            "Looks good. Send it. Send it.",
            &["send it".to_string()],
            &[],
        )
        .expect("repeated tail phrase should trigger");

        assert_eq!(result.cleaned_text, "Looks good.");
        assert_eq!(result.matched_phrase, "send it + send it");
    }

    #[test]
    fn voice_finish_trigger_matches_common_dictation_misrecognition() {
        let result = detect_voice_finish_trigger(
            "Looks like it appears to be working and dictation. End dictatation",
            &[
                "finish dictation".to_string(),
                "end dictation".to_string(),
                "send it".to_string(),
            ],
            &[],
        )
        .expect("common finish phrase misrecognition should trigger");

        assert_eq!(
            result.cleaned_text,
            "Looks like it appears to be working and dictation."
        );
        assert_eq!(result.matched_phrase, "end dictatation");
    }

    #[test]
    fn voice_finish_trigger_ignores_middle_phrase() {
        assert_eq!(
            detect_voice_finish_trigger(
                "Please write the words end dictation in the middle and keep going",
                &["end dictation".to_string()],
                &[],
            ),
            None
        );
    }

    #[test]
    fn final_text_cleanup_removes_voice_finish_trigger_before_paste() {
        let mut settings = get_default_settings();
        settings.voice_finish_trigger_enabled = true;
        settings.voice_finish_phrases = vec![
            "finish dictation".to_string(),
            "end dictation".to_string(),
            "send it".to_string(),
        ];

        assert_eq!(
            cleanup_final_text_for_paste(
                "It appears to be working and dictation. Send it. Finish dictation.",
                &settings
            ),
            "It appears to be working and dictation."
        );
    }

    #[test]
    fn final_only_mode_disables_live_progressive_updates() {
        assert!(!live_progressive_enabled_for_mode(
            DictationStabilityMode::FinalOnly
        ));
        assert!(live_progressive_enabled_for_mode(
            DictationStabilityMode::FastLive
        ));
        assert!(live_progressive_enabled_for_mode(
            DictationStabilityMode::StableLive
        ));
    }
}
