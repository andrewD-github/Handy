use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use tauri::AppHandle;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TargetIdentity {
    foreground_window: isize,
    focused_control: isize,
    interaction_epoch: u64,
}

impl TargetIdentity {
    pub(crate) fn from_raw(
        foreground_window: isize,
        focused_control: isize,
        interaction_epoch: u64,
    ) -> Self {
        Self {
            foreground_window,
            focused_control,
            interaction_epoch,
        }
    }

    fn matches(self, current: TargetIdentity) -> bool {
        self == current
    }

    #[cfg(test)]
    fn test(foreground_window: isize, focused_control: isize) -> Self {
        Self {
            foreground_window,
            focused_control,
            interaction_epoch: 0,
        }
    }

    #[cfg(test)]
    fn test_with_interaction(
        foreground_window: isize,
        focused_control: isize,
        interaction_epoch: u64,
    ) -> Self {
        Self {
            foreground_window,
            focused_control,
            interaction_epoch,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProgressiveSkipReason {
    Duplicate,
    StaleGeneration,
    ContradictoryCommit,
    Stopped,
    TargetChanged,
    EditFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProgressiveDecision {
    Append(String),
    Skip(ProgressiveSkipReason),
    FinalConflict {
        committed: String,
        final_text: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProgressiveCompletion {
    pub(crate) decision: ProgressiveDecision,
    pub(crate) owns_output: bool,
}

pub(crate) struct ProgressiveSession {
    generation: u64,
    #[allow(dead_code)]
    target: TargetIdentity,
    committed: String,
    stopped: bool,
}

impl ProgressiveSession {
    pub(crate) fn start(generation: u64, target: TargetIdentity) -> Self {
        Self {
            generation,
            target,
            committed: String::new(),
            stopped: false,
        }
    }

    pub(crate) fn apply_snapshot(
        &mut self,
        generation: u64,
        committed: &str,
        _tentative: &str,
    ) -> ProgressiveDecision {
        if self.stopped {
            return ProgressiveDecision::Skip(ProgressiveSkipReason::Stopped);
        }
        if generation != self.generation {
            return ProgressiveDecision::Skip(ProgressiveSkipReason::StaleGeneration);
        }
        if committed == self.committed {
            return ProgressiveDecision::Skip(ProgressiveSkipReason::Duplicate);
        }
        let Some(suffix) = committed.strip_prefix(&self.committed) else {
            return ProgressiveDecision::Skip(ProgressiveSkipReason::ContradictoryCommit);
        };

        self.committed.push_str(suffix);
        ProgressiveDecision::Append(suffix.to_string())
    }

    pub(crate) fn finish(&mut self, generation: u64, final_text: &str) -> ProgressiveDecision {
        if self.stopped {
            return ProgressiveDecision::Skip(ProgressiveSkipReason::Stopped);
        }
        if generation != self.generation {
            return ProgressiveDecision::Skip(ProgressiveSkipReason::StaleGeneration);
        }

        self.stopped = true;
        if final_text == self.committed {
            return ProgressiveDecision::Skip(ProgressiveSkipReason::Duplicate);
        }
        if let Some(suffix) = final_text.strip_prefix(&self.committed) {
            self.committed.push_str(suffix);
            return ProgressiveDecision::Append(suffix.to_string());
        }

        ProgressiveDecision::FinalConflict {
            committed: self.committed.clone(),
            final_text: final_text.to_string(),
        }
    }

    fn target(&self) -> TargetIdentity {
        self.target
    }

    fn generation(&self) -> u64 {
        self.generation
    }

    fn cancel(&mut self) {
        self.stopped = true;
    }
}

pub(crate) trait PromptEditor {
    fn current_target(&self) -> Option<TargetIdentity>;
    fn append(&self, text: &str) -> Result<(), String>;
}

pub(crate) struct RuntimePromptEditor {
    app: AppHandle,
}

impl RuntimePromptEditor {
    pub(crate) fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl PromptEditor for RuntimePromptEditor {
    fn current_target(&self) -> Option<TargetIdentity> {
        crate::input::capture_target_identity()
    }

    fn append(&self, text: &str) -> Result<(), String> {
        let (sender, receiver) = std::sync::mpsc::channel();
        let app = self.app.clone();
        let insertion = text.to_string();
        self.app
            .run_on_main_thread(move || {
                let result = crate::clipboard::insert_progressive_text(&insertion, &app);
                let _ = sender.send(result);
            })
            .map_err(|error| format!("Failed to schedule progressive insertion: {error:?}"))?;
        receiver
            .recv()
            .map_err(|error| format!("Failed to receive progressive insertion result: {error}"))?
    }
}

pub(crate) struct ProgressiveCoordinator<E> {
    editor: E,
    session: Option<ProgressiveSession>,
    owns_output: bool,
}

impl<E: PromptEditor> ProgressiveCoordinator<E> {
    pub(crate) fn new(editor: E) -> Self {
        Self {
            editor,
            session: None,
            owns_output: false,
        }
    }

    pub(crate) fn start(&mut self, generation: u64, target: TargetIdentity) {
        self.session = Some(ProgressiveSession::start(generation, target));
        self.owns_output = true;
    }

    pub(crate) fn apply_snapshot(
        &mut self,
        generation: u64,
        committed: &str,
        tentative: &str,
    ) -> ProgressiveDecision {
        let Some(session) = self.session.as_mut() else {
            return ProgressiveDecision::Skip(ProgressiveSkipReason::Stopped);
        };
        let decision = session.apply_snapshot(generation, committed, tentative);
        let ProgressiveDecision::Append(text) = &decision else {
            return decision;
        };

        if !self
            .editor
            .current_target()
            .is_some_and(|current| session.target().matches(current))
        {
            session.cancel();
            return ProgressiveDecision::Skip(ProgressiveSkipReason::TargetChanged);
        }
        if self.editor.append(text).is_err() {
            session.cancel();
            return ProgressiveDecision::Skip(ProgressiveSkipReason::EditFailed);
        }

        decision
    }

    pub(crate) fn finish(&mut self, generation: u64, final_text: &str) -> ProgressiveCompletion {
        let Some(session) = self.session.as_mut() else {
            return ProgressiveCompletion {
                decision: ProgressiveDecision::Skip(ProgressiveSkipReason::Stopped),
                owns_output: false,
            };
        };
        if session.generation() != generation {
            return ProgressiveCompletion {
                decision: ProgressiveDecision::Skip(ProgressiveSkipReason::StaleGeneration),
                owns_output: false,
            };
        }
        let decision = session.finish(generation, final_text);
        let ProgressiveDecision::Append(text) = &decision else {
            return ProgressiveCompletion {
                decision,
                owns_output: self.owns_output,
            };
        };

        if !self
            .editor
            .current_target()
            .is_some_and(|current| session.target().matches(current))
        {
            return ProgressiveCompletion {
                decision: ProgressiveDecision::Skip(ProgressiveSkipReason::TargetChanged),
                owns_output: self.owns_output,
            };
        }
        if self.editor.append(text).is_err() {
            return ProgressiveCompletion {
                decision: ProgressiveDecision::Skip(ProgressiveSkipReason::EditFailed),
                owns_output: self.owns_output,
            };
        }

        ProgressiveCompletion {
            decision,
            owns_output: self.owns_output,
        }
    }

    pub(crate) fn cancel(&mut self, generation: u64) {
        if let Some(session) = self.session.as_mut() {
            if session.generation() == generation {
                session.cancel();
                self.owns_output = false;
            }
        }
    }
}

pub(crate) struct ProgressiveDictationManager {
    app: AppHandle,
    coordinator: Mutex<ProgressiveCoordinator<RuntimePromptEditor>>,
    next_generation: AtomicU64,
    active_generation: AtomicU64,
    diagnostic: Mutex<Option<(u64, crate::diagnostics::DiagnosticSession)>>,
}

impl ProgressiveDictationManager {
    pub(crate) fn new(app: AppHandle) -> Self {
        Self {
            app: app.clone(),
            coordinator: Mutex::new(ProgressiveCoordinator::new(RuntimePromptEditor::new(app))),
            next_generation: AtomicU64::new(1),
            active_generation: AtomicU64::new(0),
            diagnostic: Mutex::new(None),
        }
    }

    pub(crate) fn start(&self) -> Option<u64> {
        let Some(target) = crate::input::capture_target_identity() else {
            log::warn!("Direct-prompt session not started: target identity unavailable");
            self.active_generation.store(0, Ordering::Release);
            return None;
        };
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        self.coordinator.lock().unwrap().start(generation, target);
        self.active_generation.store(generation, Ordering::Release);
        let settings = crate::settings::get_settings(&self.app);
        *self.diagnostic.lock().unwrap() =
            crate::diagnostics::DiagnosticSession::start(&self.app, generation, &settings)
                .map(|session| (generation, session));
        Some(generation)
    }

    pub(crate) fn active_generation(&self) -> Option<u64> {
        let generation = self.active_generation.load(Ordering::Acquire);
        (generation != 0).then_some(generation)
    }

    pub(crate) fn apply_snapshot(&self, generation: u64, committed: &str, tentative: &str) {
        let decision = self
            .coordinator
            .lock()
            .unwrap()
            .apply_snapshot(generation, committed, tentative);
        log::debug!(
            "Progressive snapshot decision: generation={generation}, committed_chars={}, tentative_chars={}, decision={decision:?}",
            committed.chars().count(),
            tentative.chars().count(),
        );
        if let Some((diagnostic_generation, diagnostic)) = self.diagnostic.lock().unwrap().as_ref()
        {
            if *diagnostic_generation == generation {
                let mut payload = crate::diagnostics::snapshot_metadata(
                    generation,
                    committed,
                    tentative,
                    &format!("{decision:?}"),
                );
                if let Some(fields) = payload.as_object_mut() {
                    fields.insert("committed".into(), committed.into());
                    fields.insert("tentative".into(), tentative.into());
                }
                diagnostic.record_json("stream_snapshot", payload);
            }
        }
    }

    pub(crate) fn finish(&self, generation: u64, final_text: &str) -> ProgressiveCompletion {
        let completion = self
            .coordinator
            .lock()
            .unwrap()
            .finish(generation, final_text);
        let _ = self.active_generation.compare_exchange(
            generation,
            0,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        log::debug!(
            "Progressive final decision: generation={generation}, final_chars={}, owns_output={}, decision={:?}",
            final_text.chars().count(),
            completion.owns_output,
            completion.decision,
        );
        let diagnostic = {
            let mut guard = self.diagnostic.lock().unwrap();
            if guard
                .as_ref()
                .is_some_and(|(diagnostic_generation, _)| *diagnostic_generation == generation)
            {
                guard.take().map(|(_, diagnostic)| diagnostic)
            } else {
                None
            }
        };
        if let Some(diagnostic) = diagnostic {
            diagnostic.record_json(
                "finalization",
                serde_json::json!({
                    "generation": generation,
                    "final_text": final_text,
                    "final_chars": final_text.chars().count(),
                    "owns_output": completion.owns_output,
                    "decision": format!("{:?}", completion.decision),
                }),
            );
        }
        completion
    }

    pub(crate) fn cancel(&self, generation: u64) {
        self.coordinator.lock().unwrap().cancel(generation);
        let _ = self.active_generation.compare_exchange(
            generation,
            0,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        let diagnostic = {
            let mut guard = self.diagnostic.lock().unwrap();
            if guard
                .as_ref()
                .is_some_and(|(diagnostic_generation, _)| *diagnostic_generation == generation)
            {
                guard.take().map(|(_, diagnostic)| diagnostic)
            } else {
                None
            }
        };
        if let Some(diagnostic) = diagnostic {
            diagnostic.record_json(
                "session_cancel",
                serde_json::json!({"generation": generation}),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ProgressiveCompletion, ProgressiveCoordinator, ProgressiveDecision, ProgressiveSession,
        ProgressiveSkipReason, PromptEditor, TargetIdentity,
    };
    use std::sync::{Arc, Mutex};

    struct FakeEditor {
        current_target: Mutex<Option<TargetIdentity>>,
        inserts: Mutex<Vec<String>>,
    }

    impl FakeEditor {
        fn new(target: TargetIdentity) -> Self {
            Self {
                current_target: Mutex::new(Some(target)),
                inserts: Mutex::new(Vec::new()),
            }
        }

        fn set_target(&self, target: TargetIdentity) {
            *self.current_target.lock().unwrap() = Some(target);
        }

        fn inserts(&self) -> Vec<String> {
            self.inserts.lock().unwrap().clone()
        }
    }

    impl PromptEditor for Arc<FakeEditor> {
        fn current_target(&self) -> Option<TargetIdentity> {
            *self.current_target.lock().unwrap()
        }

        fn append(&self, text: &str) -> Result<(), String> {
            self.inserts.lock().unwrap().push(text.to_string());
            Ok(())
        }
    }

    #[test]
    fn target_identity_requires_same_window_and_focused_control() {
        let expected = TargetIdentity::test(10, 20);

        assert!(expected.matches(TargetIdentity::test(10, 20)));
        assert!(!expected.matches(TargetIdentity::test(11, 20)));
        assert!(!expected.matches(TargetIdentity::test(10, 30)));
    }

    #[test]
    fn pointer_click_after_session_start_invalidates_the_target() {
        let expected = TargetIdentity::test_with_interaction(10, 20, 4);

        assert!(!expected.matches(TargetIdentity::test_with_interaction(10, 20, 5)));
    }

    #[test]
    fn coordinator_executes_an_accepted_append_once() {
        let target = TargetIdentity::test(10, 20);
        let editor = Arc::new(FakeEditor::new(target));
        let mut coordinator = ProgressiveCoordinator::new(Arc::clone(&editor));
        coordinator.start(7, target);

        assert_eq!(
            coordinator.apply_snapshot(7, "hello", ""),
            ProgressiveDecision::Append("hello".into())
        );
        assert_eq!(editor.inserts(), vec!["hello"]);
    }

    #[test]
    fn coordinator_never_executes_a_stale_generation() {
        let target = TargetIdentity::test(10, 20);
        let editor = Arc::new(FakeEditor::new(target));
        let mut coordinator = ProgressiveCoordinator::new(Arc::clone(&editor));
        coordinator.start(7, target);

        assert_eq!(
            coordinator.apply_snapshot(6, "hello", ""),
            ProgressiveDecision::Skip(ProgressiveSkipReason::StaleGeneration)
        );
        assert!(editor.inserts().is_empty());
    }

    #[test]
    fn target_change_invalidates_the_session_before_editing() {
        let target = TargetIdentity::test(10, 20);
        let editor = Arc::new(FakeEditor::new(target));
        let mut coordinator = ProgressiveCoordinator::new(Arc::clone(&editor));
        coordinator.start(7, target);
        editor.set_target(TargetIdentity::test(10, 30));

        assert_eq!(
            coordinator.apply_snapshot(7, "hello", ""),
            ProgressiveDecision::Skip(ProgressiveSkipReason::TargetChanged)
        );
        assert!(editor.inserts().is_empty());
        assert_eq!(
            coordinator.apply_snapshot(7, "hello world", ""),
            ProgressiveDecision::Skip(ProgressiveSkipReason::Stopped)
        );
    }

    #[test]
    fn final_tail_is_inserted_once_and_marks_output_owned() {
        let target = TargetIdentity::test(10, 20);
        let editor = Arc::new(FakeEditor::new(target));
        let mut coordinator = ProgressiveCoordinator::new(Arc::clone(&editor));
        coordinator.start(7, target);
        coordinator.apply_snapshot(7, "hello", "");

        assert_eq!(
            coordinator.finish(7, "hello world"),
            ProgressiveCompletion {
                decision: ProgressiveDecision::Append(" world".into()),
                owns_output: true,
            }
        );
        assert_eq!(editor.inserts(), vec!["hello", " world"]);
    }

    #[test]
    fn new_generation_makes_old_updates_and_finalization_stale() {
        let target = TargetIdentity::test(10, 20);
        let editor = Arc::new(FakeEditor::new(target));
        let mut coordinator = ProgressiveCoordinator::new(Arc::clone(&editor));
        coordinator.start(7, target);
        coordinator.start(8, target);

        assert_eq!(
            coordinator.apply_snapshot(7, "old text", ""),
            ProgressiveDecision::Skip(ProgressiveSkipReason::StaleGeneration)
        );
        assert_eq!(
            coordinator.finish(7, "old text"),
            ProgressiveCompletion {
                decision: ProgressiveDecision::Skip(ProgressiveSkipReason::StaleGeneration),
                owns_output: false,
            }
        );
        assert!(editor.inserts().is_empty());
    }

    #[test]
    fn cancelled_session_never_edits_or_owns_output() {
        let target = TargetIdentity::test(10, 20);
        let editor = Arc::new(FakeEditor::new(target));
        let mut coordinator = ProgressiveCoordinator::new(Arc::clone(&editor));
        coordinator.start(7, target);
        coordinator.cancel(7);

        assert_eq!(
            coordinator.apply_snapshot(7, "late text", ""),
            ProgressiveDecision::Skip(ProgressiveSkipReason::Stopped)
        );
        assert_eq!(
            coordinator.finish(7, "late text"),
            ProgressiveCompletion {
                decision: ProgressiveDecision::Skip(ProgressiveSkipReason::Stopped),
                owns_output: false,
            }
        );
        assert!(editor.inserts().is_empty());
    }

    #[test]
    fn target_change_after_live_insert_blocks_final_fallback() {
        let target = TargetIdentity::test(10, 20);
        let editor = Arc::new(FakeEditor::new(target));
        let mut coordinator = ProgressiveCoordinator::new(Arc::clone(&editor));
        coordinator.start(7, target);
        coordinator.apply_snapshot(7, "hello", "");
        editor.set_target(TargetIdentity::test(10, 30));

        assert_eq!(
            coordinator.finish(7, "hello world"),
            ProgressiveCompletion {
                decision: ProgressiveDecision::Skip(ProgressiveSkipReason::TargetChanged),
                owns_output: true,
            }
        );
        assert_eq!(editor.inserts(), vec!["hello"]);
    }

    #[test]
    fn target_change_before_first_insert_still_blocks_final_fallback() {
        let target = TargetIdentity::test(10, 20);
        let editor = Arc::new(FakeEditor::new(target));
        let mut coordinator = ProgressiveCoordinator::new(Arc::clone(&editor));
        coordinator.start(7, target);
        editor.set_target(TargetIdentity::test(10, 30));

        assert_eq!(
            coordinator.finish(7, "hello"),
            ProgressiveCompletion {
                decision: ProgressiveDecision::Skip(ProgressiveSkipReason::TargetChanged),
                owns_output: true,
            }
        );
        assert!(editor.inserts().is_empty());
    }

    #[test]
    fn first_committed_snapshot_appends_full_prefix() {
        let mut session = ProgressiveSession::start(7, TargetIdentity::test(10, 20));

        assert_eq!(
            session.apply_snapshot(7, "hello", " wor"),
            ProgressiveDecision::Append("hello".into())
        );
    }

    #[test]
    fn growing_committed_snapshot_appends_only_suffix() {
        let mut session = ProgressiveSession::start(7, TargetIdentity::test(10, 20));
        session.apply_snapshot(7, "hello", "");

        assert_eq!(
            session.apply_snapshot(7, "hello world", ""),
            ProgressiveDecision::Append(" world".into())
        );
    }

    #[test]
    fn tentative_text_is_never_returned_as_a_prompt_edit() {
        let mut session = ProgressiveSession::start(7, TargetIdentity::test(10, 20));

        assert_eq!(
            session.apply_snapshot(7, "hello", " volatile words"),
            ProgressiveDecision::Append("hello".into())
        );
        assert_eq!(
            session.apply_snapshot(7, "hello", " different volatile words"),
            ProgressiveDecision::Skip(ProgressiveSkipReason::Duplicate)
        );
    }

    #[test]
    fn stale_or_contradictory_snapshot_never_edits_prompt() {
        let mut session = ProgressiveSession::start(7, TargetIdentity::test(10, 20));
        session.apply_snapshot(7, "hello world", "");

        assert_eq!(
            session.apply_snapshot(6, "hello world!", ""),
            ProgressiveDecision::Skip(ProgressiveSkipReason::StaleGeneration)
        );
        assert_eq!(
            session.apply_snapshot(7, "hullo world", ""),
            ProgressiveDecision::Skip(ProgressiveSkipReason::ContradictoryCommit)
        );
    }

    #[test]
    fn final_extension_appends_tail_but_material_disagreement_is_reported() {
        let mut session = ProgressiveSession::start(7, TargetIdentity::test(10, 20));
        session.apply_snapshot(7, "hello", "");
        assert_eq!(
            session.finish(7, "hello world"),
            ProgressiveDecision::Append(" world".into())
        );

        let mut conflict = ProgressiveSession::start(8, TargetIdentity::test(10, 20));
        conflict.apply_snapshot(8, "hello", "");
        assert_eq!(
            conflict.finish(8, "hullo"),
            ProgressiveDecision::FinalConflict {
                committed: "hello".into(),
                final_text: "hullo".into(),
            }
        );
    }

    #[test]
    fn finish_invalidates_the_session() {
        let mut session = ProgressiveSession::start(7, TargetIdentity::test(10, 20));
        session.finish(7, "hello");

        assert_eq!(
            session.apply_snapshot(7, "hello world", ""),
            ProgressiveDecision::Skip(ProgressiveSkipReason::Stopped)
        );
    }
}
