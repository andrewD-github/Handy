#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TargetIdentity {
    foreground_window: isize,
    focused_control: isize,
}

impl TargetIdentity {
    pub(crate) fn from_raw(foreground_window: isize, focused_control: isize) -> Self {
        Self {
            foreground_window,
            focused_control,
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

    pub(crate) fn finish(
        &mut self,
        generation: u64,
        final_text: &str,
    ) -> ProgressiveDecision {
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
}

impl<E: PromptEditor> ProgressiveCoordinator<E> {
    pub(crate) fn new(editor: E) -> Self {
        Self {
            editor,
            session: None,
        }
    }

    pub(crate) fn start(&mut self, generation: u64, target: TargetIdentity) {
        self.session = Some(ProgressiveSession::start(generation, target));
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
}

#[cfg(test)]
mod tests {
    use super::{
        ProgressiveCoordinator, ProgressiveDecision, ProgressiveSession, ProgressiveSkipReason,
        PromptEditor, TargetIdentity,
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
use tauri::AppHandle;
