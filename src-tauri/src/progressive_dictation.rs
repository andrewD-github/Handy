#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TargetIdentity {
    foreground_window: isize,
    focused_control: isize,
}

impl TargetIdentity {
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
}

#[cfg(test)]
mod tests {
    use super::{ProgressiveDecision, ProgressiveSession, ProgressiveSkipReason, TargetIdentity};

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
