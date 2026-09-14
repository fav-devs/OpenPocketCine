//! Leaving playback and bringing live view back — Mimo's "Back to live view".
//!
//! Transcribed from `MediaLiveResume` and `MediaBrowsePolicy` in the core. Enable
//! (`0x09/0xa8`) while still in playback ACKs `E0`/`D6` and produces no video, so the
//! shell keeps exiting until the `0x02/0x80` playback bit clears, then enables.

pub const MAX_EXIT_ATTEMPTS: u32 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeAction {
    ExitPlayback,
    EnableLiveView,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResumePolicy;

impl ResumePolicy {
    /// One tick of the post-media resume loop.
    pub fn action(
        attempt: u32,
        in_playback: bool,
        exit_acknowledged: bool,
        picture_fresh: bool,
    ) -> ResumeAction {
        if picture_fresh && !in_playback {
            return ResumeAction::Done;
        }
        if in_playback || !exit_acknowledged {
            return ResumeAction::ExitPlayback;
        }
        if attempt > MAX_EXIT_ATTEMPTS {
            return if picture_fresh {
                ResumeAction::Done
            } else {
                ResumeAction::EnableLiveView
            };
        }
        ResumeAction::EnableLiveView
    }

    /// Keepalive while the operator is on live view but the camera still says playback.
    pub fn stray_playback_action(browsing: bool, in_playback: bool) -> Option<ResumeAction> {
        (!browsing && in_playback).then_some(ResumeAction::ExitPlayback)
    }

    /// Leftover GOP packets are not a live picture: only a frame presented after the
    /// resume started counts.
    pub fn is_picture_fresh(last_presented_at: Option<f64>, since: f64) -> bool {
        last_presented_at.is_some_and(|at| at >= since)
    }
}

/// What to do after `0x02/0x0c` enter-playback. The newest page needs no playback, so
/// a Pocket 3 that answers `E0` after a take still lists; only older pages need it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrowsePolicy {
    pub list_newest_page: bool,
    pub list_older_pages: bool,
    pub keep_browsing: bool,
}

impl BrowsePolicy {
    pub fn after_enter_playback(entered: bool) -> Self {
        Self {
            list_newest_page: true,
            list_older_pages: entered,
            keep_browsing: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_exits_until_the_bit_clears_then_enables() {
        assert_eq!(
            ResumePolicy::action(0, true, false, false),
            ResumeAction::ExitPlayback
        );
        assert_eq!(
            ResumePolicy::action(1, false, false, false),
            ResumeAction::ExitPlayback
        );
        assert_eq!(
            ResumePolicy::action(1, false, true, false),
            ResumeAction::EnableLiveView
        );
        assert_eq!(
            ResumePolicy::action(1, false, true, true),
            ResumeAction::Done
        );
        assert_eq!(
            ResumePolicy::action(9, false, true, false),
            ResumeAction::EnableLiveView
        );
    }

    #[test]
    fn a_stray_playback_is_exited_only_off_the_browser() {
        assert_eq!(
            ResumePolicy::stray_playback_action(false, true),
            Some(ResumeAction::ExitPlayback)
        );
        assert_eq!(ResumePolicy::stray_playback_action(true, true), None);
        assert_eq!(ResumePolicy::stray_playback_action(false, false), None);
    }

    #[test]
    fn a_failed_entry_still_lists_the_newest_page() {
        let policy = BrowsePolicy::after_enter_playback(false);
        assert!(policy.list_newest_page && !policy.list_older_pages && policy.keep_browsing);
        assert!(ResumePolicy::is_picture_fresh(Some(5.0), 4.0));
        assert!(!ResumePolicy::is_picture_fresh(Some(3.0), 4.0));
        assert!(!ResumePolicy::is_picture_fresh(None, 4.0));
    }
}
