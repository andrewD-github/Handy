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

    pub(crate) fn matches(self, current: TargetIdentity) -> bool {
        self == current
    }

    pub(crate) fn same_native_target(self, current: TargetIdentity) -> bool {
        self.foreground_window == current.foreground_window
            && self.focused_control == current.focused_control
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
    ContradictoryDisplay,
    Stopped,
    TargetChanged,
    EditFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProgressiveDecision {
    Append(String),
    ResumeAppend(String),
    Paused,
    Skip(ProgressiveSkipReason),
    FinalConflict {
        displayed: String,
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
    target: TargetIdentity,
    displayed: String,
    stopped: bool,
}

impl ProgressiveSession {
    pub(crate) fn start(generation: u64, target: TargetIdentity) -> Self {
        Self {
            generation,
            target,
            displayed: String::new(),
            stopped: false,
        }
    }

    pub(crate) fn apply_snapshot(
        &mut self,
        generation: u64,
        committed: &str,
        tentative: &str,
    ) -> ProgressiveDecision {
        if self.stopped {
            return ProgressiveDecision::Skip(ProgressiveSkipReason::Stopped);
        }
        if generation != self.generation {
            return ProgressiveDecision::Skip(ProgressiveSkipReason::StaleGeneration);
        }
        let snapshot = format!("{committed}{tentative}");
        if snapshot == self.displayed {
            return ProgressiveDecision::Skip(ProgressiveSkipReason::Duplicate);
        }
        let Some(suffix) = snapshot.strip_prefix(&self.displayed) else {
            return ProgressiveDecision::Skip(ProgressiveSkipReason::ContradictoryDisplay);
        };

        self.displayed.push_str(suffix);
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
        if final_text == self.displayed {
            return ProgressiveDecision::Skip(ProgressiveSkipReason::Duplicate);
        }
        if let Some(suffix) = final_text.strip_prefix(&self.displayed) {
            self.displayed.push_str(suffix);
            return ProgressiveDecision::Append(suffix.to_string());
        }

        ProgressiveDecision::FinalConflict {
            displayed: self.displayed.clone(),
            final_text: final_text.to_string(),
        }
    }

    fn target(&self) -> TargetIdentity {
        self.target
    }

    fn set_target(&mut self, target: TargetIdentity) {
        self.target = target;
    }

    fn displayed(&self) -> &str {
        &self.displayed
    }

    fn generation(&self) -> u64 {
        self.generation
    }

    fn cancel(&mut self) {
        self.stopped = true;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PromptEditError {
    TargetChanged,
    EditFailed,
    PartialEdit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RefocusPolicy {
    Stop,
    ResumeOriginalTarget,
}

pub(crate) trait PromptEditor {
    fn current_target(&self) -> Option<TargetIdentity>;

    fn append_verified(
        &self,
        expected_target: TargetIdentity,
        text: &str,
    ) -> Result<(), PromptEditError>;
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

    fn append_verified(
        &self,
        expected_target: TargetIdentity,
        text: &str,
    ) -> Result<(), PromptEditError> {
        let (sender, receiver) = std::sync::mpsc::channel();
        let insertion = text.to_string();
        self.app
            .run_on_main_thread(move || {
                let result =
                    crate::input::insert_text_at_verified_target(expected_target, &insertion)
                        .map_err(|error| match error {
                            crate::input::VerifiedTargetError::TargetChanged
                            | crate::input::VerifiedTargetError::MonitorUnavailable => {
                                PromptEditError::TargetChanged
                            }
                            crate::input::VerifiedTargetError::Action(_) => {
                                PromptEditError::EditFailed
                            }
                            crate::input::VerifiedTargetError::PartialAction(_) => {
                                PromptEditError::PartialEdit
                            }
                        });
                let _ = sender.send(result);
            })
            .map_err(|_| PromptEditError::EditFailed)?;
        receiver.recv().map_err(|_| PromptEditError::EditFailed)?
    }
}

pub(crate) struct ProgressiveCoordinator<E> {
    editor: E,
    session: Option<ProgressiveSession>,
    inserted_any: bool,
    inserted: String,
    unsafe_target: bool,
    refocus_policy: RefocusPolicy,
    paused: bool,
}

impl<E: PromptEditor> ProgressiveCoordinator<E> {
    pub(crate) fn new(editor: E) -> Self {
        Self {
            editor,
            session: None,
            inserted_any: false,
            inserted: String::new(),
            unsafe_target: false,
            refocus_policy: RefocusPolicy::Stop,
            paused: false,
        }
    }

    pub(crate) fn start(
        &mut self,
        generation: u64,
        target: TargetIdentity,
        refocus_policy: RefocusPolicy,
    ) {
        self.session = Some(ProgressiveSession::start(generation, target));
        self.inserted_any = false;
        self.inserted.clear();
        self.unsafe_target = false;
        self.refocus_policy = refocus_policy;
        self.paused = false;
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
        let snapshot_decision = session.apply_snapshot(generation, committed, tentative);
        if !matches!(
            snapshot_decision,
            ProgressiveDecision::Append(_)
                | ProgressiveDecision::Skip(ProgressiveSkipReason::Duplicate)
        ) {
            return snapshot_decision;
        }

        let Some(current_target) = self.editor.current_target() else {
            return self.handle_target_change();
        };
        let exact_match = session.target().matches(current_target);
        let same_native_target = session.target().same_native_target(current_target);
        let resumed = self.paused || !exact_match;
        match self.refocus_policy {
            RefocusPolicy::Stop if !exact_match => return self.handle_target_change(),
            RefocusPolicy::ResumeOriginalTarget if !same_native_target => {
                self.paused = true;
                self.unsafe_target = true;
                return ProgressiveDecision::Paused;
            }
            RefocusPolicy::ResumeOriginalTarget => session.set_target(current_target),
            RefocusPolicy::Stop => {}
        }

        let Some(text) = session.displayed().strip_prefix(&self.inserted) else {
            session.cancel();
            return ProgressiveDecision::Skip(ProgressiveSkipReason::ContradictoryDisplay);
        };
        if text.is_empty() {
            self.paused = false;
            return snapshot_decision;
        }
        let text = text.to_string();

        match self.editor.append_verified(session.target(), &text) {
            Ok(()) => {
                self.inserted_any = true;
                self.inserted.push_str(&text);
                self.paused = false;
            }
            Err(PromptEditError::TargetChanged) => {
                return self.handle_target_change();
            }
            Err(PromptEditError::EditFailed) => {
                session.cancel();
                return ProgressiveDecision::Skip(ProgressiveSkipReason::EditFailed);
            }
            Err(PromptEditError::PartialEdit) => {
                session.cancel();
                self.inserted_any = true;
                return ProgressiveDecision::Skip(ProgressiveSkipReason::EditFailed);
            }
        }

        if resumed {
            ProgressiveDecision::ResumeAppend(text)
        } else {
            ProgressiveDecision::Append(text)
        }
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
        let final_decision = session.finish(generation, final_text);
        if !matches!(
            final_decision,
            ProgressiveDecision::Append(_)
                | ProgressiveDecision::Skip(ProgressiveSkipReason::Duplicate)
        ) {
            return ProgressiveCompletion {
                decision: final_decision,
                owns_output: self.inserted_any || self.unsafe_target,
            };
        }

        let Some(current_target) = self.editor.current_target() else {
            self.unsafe_target = true;
            return ProgressiveCompletion {
                decision: ProgressiveDecision::Skip(ProgressiveSkipReason::TargetChanged),
                owns_output: true,
            };
        };
        let exact_match = session.target().matches(current_target);
        let same_native_target = session.target().same_native_target(current_target);
        let resumed = self.paused || !exact_match;
        let target_is_eligible = match self.refocus_policy {
            RefocusPolicy::Stop => exact_match,
            RefocusPolicy::ResumeOriginalTarget => same_native_target,
        };
        if !target_is_eligible {
            self.unsafe_target = true;
            return ProgressiveCompletion {
                decision: ProgressiveDecision::Skip(ProgressiveSkipReason::TargetChanged),
                owns_output: true,
            };
        }
        if self.refocus_policy == RefocusPolicy::ResumeOriginalTarget {
            session.set_target(current_target);
        }
        let Some(text) = session.displayed().strip_prefix(&self.inserted) else {
            return ProgressiveCompletion {
                decision: ProgressiveDecision::Skip(ProgressiveSkipReason::ContradictoryDisplay),
                owns_output: self.inserted_any || self.unsafe_target,
            };
        };
        if text.is_empty() {
            return ProgressiveCompletion {
                decision: final_decision,
                owns_output: self.inserted_any || self.unsafe_target,
            };
        }
        let text = text.to_string();

        match self.editor.append_verified(session.target(), &text) {
            Ok(()) => {
                self.inserted_any = true;
                self.inserted.push_str(&text);
                self.paused = false;
            }
            Err(PromptEditError::TargetChanged) => {
                self.unsafe_target = true;
                return ProgressiveCompletion {
                    decision: ProgressiveDecision::Skip(ProgressiveSkipReason::TargetChanged),
                    owns_output: true,
                };
            }
            Err(PromptEditError::EditFailed) => {
                return ProgressiveCompletion {
                    decision: ProgressiveDecision::Skip(ProgressiveSkipReason::EditFailed),
                    owns_output: self.inserted_any,
                };
            }
            Err(PromptEditError::PartialEdit) => {
                self.inserted_any = true;
                return ProgressiveCompletion {
                    decision: ProgressiveDecision::Skip(ProgressiveSkipReason::EditFailed),
                    owns_output: true,
                };
            }
        }

        ProgressiveCompletion {
            decision: if resumed {
                ProgressiveDecision::ResumeAppend(text)
            } else {
                ProgressiveDecision::Append(text)
            },
            owns_output: self.inserted_any || self.unsafe_target,
        }
    }

    pub(crate) fn cancel(&mut self, generation: u64) {
        if let Some(session) = self.session.as_mut() {
            if session.generation() == generation {
                session.cancel();
                self.inserted_any = false;
                self.inserted.clear();
                self.unsafe_target = false;
                self.paused = false;
            }
        }
    }

    fn handle_target_change(&mut self) -> ProgressiveDecision {
        self.unsafe_target = true;
        match self.refocus_policy {
            RefocusPolicy::Stop => {
                if let Some(session) = self.session.as_mut() {
                    session.cancel();
                }
                ProgressiveDecision::Skip(ProgressiveSkipReason::TargetChanged)
            }
            RefocusPolicy::ResumeOriginalTarget => {
                self.paused = true;
                ProgressiveDecision::Paused
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

    pub(crate) fn start(&self, resume_on_refocus: bool) -> Option<u64> {
        if !crate::input::target_interaction_monitor_ready() {
            log::error!(
                "Direct-prompt session not started: target interaction monitor unavailable"
            );
            self.active_generation.store(0, Ordering::Release);
            return None;
        }
        let Some(target) = crate::input::capture_target_identity() else {
            log::warn!("Direct-prompt session not started: target identity unavailable");
            self.active_generation.store(0, Ordering::Release);
            return None;
        };
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        let policy = if resume_on_refocus {
            RefocusPolicy::ResumeOriginalTarget
        } else {
            RefocusPolicy::Stop
        };
        self.coordinator
            .lock()
            .unwrap()
            .start(generation, target, policy);
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
        ProgressiveSkipReason, PromptEditError, PromptEditor, RefocusPolicy, TargetIdentity,
    };
    use std::sync::{Arc, Mutex};

    struct FakeEditor {
        current_target: Mutex<Option<TargetIdentity>>,
        target_on_next_append: Mutex<Option<TargetIdentity>>,
        fail_next_append: Mutex<bool>,
        partially_fail_next_append: Mutex<bool>,
        inserts: Mutex<Vec<String>>,
    }

    impl FakeEditor {
        fn new(target: TargetIdentity) -> Self {
            Self {
                current_target: Mutex::new(Some(target)),
                target_on_next_append: Mutex::new(None),
                fail_next_append: Mutex::new(false),
                partially_fail_next_append: Mutex::new(false),
                inserts: Mutex::new(Vec::new()),
            }
        }

        fn set_target(&self, target: TargetIdentity) {
            *self.current_target.lock().unwrap() = Some(target);
        }

        fn inserts(&self) -> Vec<String> {
            self.inserts.lock().unwrap().clone()
        }

        fn change_target_during_next_append(&self, target: TargetIdentity) {
            *self.target_on_next_append.lock().unwrap() = Some(target);
        }

        fn fail_next_append(&self) {
            *self.fail_next_append.lock().unwrap() = true;
        }

        fn partially_fail_next_append(&self) {
            *self.partially_fail_next_append.lock().unwrap() = true;
        }
    }

    impl PromptEditor for Arc<FakeEditor> {
        fn current_target(&self) -> Option<TargetIdentity> {
            *self.current_target.lock().unwrap()
        }

        fn append_verified(
            &self,
            expected_target: TargetIdentity,
            text: &str,
        ) -> Result<(), PromptEditError> {
            if let Some(target) = self.target_on_next_append.lock().unwrap().take() {
                *self.current_target.lock().unwrap() = Some(target);
            }
            if !self
                .current_target
                .lock()
                .unwrap()
                .is_some_and(|current| expected_target.matches(current))
            {
                return Err(PromptEditError::TargetChanged);
            }
            if std::mem::take(&mut *self.fail_next_append.lock().unwrap()) {
                return Err(PromptEditError::EditFailed);
            }
            if std::mem::take(&mut *self.partially_fail_next_append.lock().unwrap()) {
                return Err(PromptEditError::PartialEdit);
            }
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
    fn resume_mode_catches_up_once_when_original_target_returns() {
        let original = TargetIdentity::test_with_interaction(10, 20, 1);
        let other = TargetIdentity::test_with_interaction(10, 30, 2);
        let returned = TargetIdentity::test_with_interaction(10, 20, 3);
        let editor = Arc::new(FakeEditor::new(original));
        let mut coordinator = ProgressiveCoordinator::new(Arc::clone(&editor));
        coordinator.start(7, original, RefocusPolicy::ResumeOriginalTarget);

        assert_eq!(
            coordinator.apply_snapshot(7, "hello", ""),
            ProgressiveDecision::Append("hello".into())
        );
        editor.set_target(other);
        assert_eq!(
            coordinator.apply_snapshot(7, "hello world", ""),
            ProgressiveDecision::Paused
        );
        assert_eq!(editor.inserts(), vec!["hello"]);

        editor.set_target(returned);
        assert_eq!(
            coordinator.apply_snapshot(7, "hello world again", ""),
            ProgressiveDecision::ResumeAppend(" world again".into())
        );
        assert_eq!(editor.inserts(), vec!["hello", " world again"]);

        assert_eq!(
            coordinator.apply_snapshot(7, "hello world again", ""),
            ProgressiveDecision::Skip(ProgressiveSkipReason::Duplicate)
        );
        assert_eq!(editor.inserts(), vec!["hello", " world again"]);
    }

    #[test]
    fn resume_mode_rearms_after_a_click_in_the_original_native_target() {
        let original = TargetIdentity::test_with_interaction(10, 20, 4);
        let returned = TargetIdentity::test_with_interaction(10, 20, 5);
        let editor = Arc::new(FakeEditor::new(original));
        let mut coordinator = ProgressiveCoordinator::new(Arc::clone(&editor));
        coordinator.start(7, original, RefocusPolicy::ResumeOriginalTarget);
        coordinator.apply_snapshot(7, "hello", "");
        editor.set_target(returned);

        assert_eq!(
            coordinator.apply_snapshot(7, "hello again", ""),
            ProgressiveDecision::ResumeAppend(" again".into())
        );
        assert_eq!(editor.inserts(), vec!["hello", " again"]);
    }

    #[test]
    fn resume_mode_never_finalizes_into_a_different_target() {
        let original = TargetIdentity::test_with_interaction(10, 20, 1);
        let other = TargetIdentity::test_with_interaction(11, 30, 2);
        let editor = Arc::new(FakeEditor::new(original));
        let mut coordinator = ProgressiveCoordinator::new(Arc::clone(&editor));
        coordinator.start(7, original, RefocusPolicy::ResumeOriginalTarget);
        coordinator.apply_snapshot(7, "hello", "");
        editor.set_target(other);
        assert_eq!(
            coordinator.apply_snapshot(7, "hello world", ""),
            ProgressiveDecision::Paused
        );

        assert_eq!(
            coordinator.finish(7, "hello world again"),
            ProgressiveCompletion {
                decision: ProgressiveDecision::Skip(ProgressiveSkipReason::TargetChanged),
                owns_output: true,
            }
        );
        assert_eq!(editor.inserts(), vec!["hello"]);
    }

    #[test]
    fn cancelled_resume_session_cannot_rearm() {
        let original = TargetIdentity::test_with_interaction(10, 20, 1);
        let other = TargetIdentity::test_with_interaction(10, 30, 2);
        let returned = TargetIdentity::test_with_interaction(10, 20, 3);
        let editor = Arc::new(FakeEditor::new(original));
        let mut coordinator = ProgressiveCoordinator::new(Arc::clone(&editor));
        coordinator.start(7, original, RefocusPolicy::ResumeOriginalTarget);
        editor.set_target(other);
        assert_eq!(
            coordinator.apply_snapshot(7, "hello", ""),
            ProgressiveDecision::Paused
        );
        coordinator.cancel(7);
        editor.set_target(returned);

        assert_eq!(
            coordinator.apply_snapshot(7, "hello world", ""),
            ProgressiveDecision::Skip(ProgressiveSkipReason::Stopped)
        );
        assert!(editor.inserts().is_empty());
    }

    #[test]
    fn coordinator_executes_an_accepted_append_once() {
        let target = TargetIdentity::test(10, 20);
        let editor = Arc::new(FakeEditor::new(target));
        let mut coordinator = ProgressiveCoordinator::new(Arc::clone(&editor));
        coordinator.start(7, target, RefocusPolicy::Stop);

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
        coordinator.start(7, target, RefocusPolicy::Stop);

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
        coordinator.start(7, target, RefocusPolicy::Stop);
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
    fn target_change_after_queueing_is_rechecked_inside_the_edit_operation() {
        let target = TargetIdentity::test(10, 20);
        let editor = Arc::new(FakeEditor::new(target));
        let mut coordinator = ProgressiveCoordinator::new(Arc::clone(&editor));
        coordinator.start(7, target, RefocusPolicy::Stop);
        editor.change_target_during_next_append(TargetIdentity::test_with_interaction(10, 20, 1));

        assert_eq!(
            coordinator.apply_snapshot(7, "late text", ""),
            ProgressiveDecision::Skip(ProgressiveSkipReason::TargetChanged)
        );
        assert!(editor.inserts().is_empty());
    }

    #[test]
    fn failed_first_edit_allows_normal_final_fallback() {
        let target = TargetIdentity::test(10, 20);
        let editor = Arc::new(FakeEditor::new(target));
        let mut coordinator = ProgressiveCoordinator::new(Arc::clone(&editor));
        coordinator.start(7, target, RefocusPolicy::Stop);
        editor.fail_next_append();

        assert_eq!(
            coordinator.apply_snapshot(7, "hello", ""),
            ProgressiveDecision::Skip(ProgressiveSkipReason::EditFailed)
        );
        assert_eq!(
            coordinator.finish(7, "hello"),
            ProgressiveCompletion {
                decision: ProgressiveDecision::Skip(ProgressiveSkipReason::Stopped),
                owns_output: false,
            }
        );
    }

    #[test]
    fn partially_inserted_first_edit_blocks_full_final_fallback() {
        let target = TargetIdentity::test(10, 20);
        let editor = Arc::new(FakeEditor::new(target));
        let mut coordinator = ProgressiveCoordinator::new(Arc::clone(&editor));
        coordinator.start(7, target, RefocusPolicy::Stop);
        editor.partially_fail_next_append();

        assert_eq!(
            coordinator.apply_snapshot(7, "hello", ""),
            ProgressiveDecision::Skip(ProgressiveSkipReason::EditFailed)
        );
        assert_eq!(
            coordinator.finish(7, "hello"),
            ProgressiveCompletion {
                decision: ProgressiveDecision::Skip(ProgressiveSkipReason::Stopped),
                owns_output: true,
            }
        );
    }

    #[test]
    fn final_tail_is_inserted_once_and_marks_output_owned() {
        let target = TargetIdentity::test(10, 20);
        let editor = Arc::new(FakeEditor::new(target));
        let mut coordinator = ProgressiveCoordinator::new(Arc::clone(&editor));
        coordinator.start(7, target, RefocusPolicy::Stop);
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
    fn final_text_is_inserted_once_when_script_previews_were_withheld() {
        let target = TargetIdentity::test(10, 20);
        let editor = Arc::new(FakeEditor::new(target));
        let mut coordinator = ProgressiveCoordinator::new(Arc::clone(&editor));
        coordinator.start(7, target, RefocusPolicy::Stop);

        assert_eq!(
            coordinator.finish(7, "我们今天去学校"),
            ProgressiveCompletion {
                decision: ProgressiveDecision::Append("我们今天去学校".into()),
                owns_output: true,
            }
        );
        assert_eq!(editor.inserts(), vec!["我们今天去学校"]);
    }

    #[test]
    fn new_generation_makes_old_updates_and_finalization_stale() {
        let target = TargetIdentity::test(10, 20);
        let editor = Arc::new(FakeEditor::new(target));
        let mut coordinator = ProgressiveCoordinator::new(Arc::clone(&editor));
        coordinator.start(7, target, RefocusPolicy::Stop);
        coordinator.start(8, target, RefocusPolicy::Stop);

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
        coordinator.start(7, target, RefocusPolicy::Stop);
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
        coordinator.start(7, target, RefocusPolicy::Stop);
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
        coordinator.start(7, target, RefocusPolicy::Stop);
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
            ProgressiveDecision::Append("hello wor".into())
        );
    }

    #[test]
    fn growing_committed_snapshot_appends_only_suffix() {
        let mut session = ProgressiveSession::start(7, TargetIdentity::test(10, 20));
        session.apply_snapshot(7, "hello", " wor");

        assert_eq!(
            session.apply_snapshot(7, "hello world", ""),
            ProgressiveDecision::Append("ld".into())
        );
    }

    #[test]
    fn growing_tentative_text_is_appended_but_a_retraction_is_never_typed() {
        let mut session = ProgressiveSession::start(7, TargetIdentity::test(10, 20));

        assert_eq!(
            session.apply_snapshot(7, "hello", " volatile words"),
            ProgressiveDecision::Append("hello volatile words".into())
        );
        assert_eq!(
            session.apply_snapshot(7, "hello", " different volatile words"),
            ProgressiveDecision::Skip(ProgressiveSkipReason::ContradictoryDisplay)
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
            ProgressiveDecision::Skip(ProgressiveSkipReason::ContradictoryDisplay)
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
                displayed: "hello".into(),
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
