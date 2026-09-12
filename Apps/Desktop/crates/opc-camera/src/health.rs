//! What the session has seen, and how long ago.
//!
//! The watchdog decides what to do about a frozen feed; this only remembers when things
//! last happened so it can be asked. Keeping the two apart means the bookkeeping — which
//! is where an off-by-one turns into a recover ladder that never fires — is testable on
//! its own, with no core and no camera.
//!
//! Ages cross the boundary as seconds, with a negative value meaning never seen. Zero is
//! a real age, so "never" and "just now" must not collapse into each other.

use opc_core_sys::OpcWatchdogSnapshot;

/// Timestamps of everything the watchdog asks about.
#[derive(Debug, Default, Clone)]
pub struct FeedHealth {
    last_video_packet: Option<f64>,
    last_access_unit: Option<f64>,
    last_decoded_frame: Option<f64>,
    last_status: Option<f64>,
    last_enable: Option<f64>,
    last_rebuild: Option<f64>,
    last_camera_set: Option<f64>,
    last_zoom_set: Option<f64>,
    last_gimbal_throw: Option<f64>,
    last_focus_track_set: Option<f64>,
    saw_picture: bool,
    has_format: bool,
    decoder_failed: bool,
    path_ready: bool,
    flow_healthy: bool,
}

impl FeedHealth {
    pub fn new() -> Self {
        Self {
            path_ready: true,
            flow_healthy: true,
            ..Self::default()
        }
    }

    pub fn note_video_packet(&mut self, now: f64) {
        self.last_video_packet = Some(now);
        self.saw_picture = true;
    }

    pub fn note_access_unit(&mut self, now: f64) {
        self.last_access_unit = Some(now);
        self.has_format = true;
    }

    /// A picture actually reached the screen, which is not the same as one arriving.
    pub fn note_decoded_frame(&mut self, now: f64) {
        self.last_decoded_frame = Some(now);
    }

    pub fn note_status(&mut self, now: f64) {
        self.last_status = Some(now);
    }

    pub fn note_enable(&mut self, now: f64) {
        self.last_enable = Some(now);
    }

    pub fn note_rebuild(&mut self, now: f64) {
        self.last_rebuild = Some(now);
    }

    pub fn note_camera_set(&mut self, now: f64) {
        self.last_camera_set = Some(now);
    }

    pub fn note_zoom(&mut self, now: f64) {
        self.last_zoom_set = Some(now);
        self.note_camera_set(now);
    }

    pub fn note_gimbal_throw(&mut self, now: f64) {
        self.last_gimbal_throw = Some(now);
    }

    pub fn note_focus_track(&mut self, now: f64) {
        self.last_focus_track_set = Some(now);
        self.note_camera_set(now);
    }

    pub fn set_path_ready(&mut self, ready: bool) {
        self.path_ready = ready;
    }

    pub fn set_flow_healthy(&mut self, healthy: bool) {
        self.flow_healthy = healthy;
    }

    pub fn set_decoder_failed(&mut self, failed: bool) {
        self.decoder_failed = failed;
    }

    pub fn saw_picture(&self) -> bool {
        self.saw_picture
    }

    /// Drops the per-attempt history a rebuild invalidates, keeping what a session knows
    /// about itself. `saw_picture` in particular must survive: the watchdog treats a feed
    /// that has never shown anything differently from one that froze.
    pub fn note_datalink_rebuilt(&mut self, now: f64) {
        self.last_video_packet = None;
        self.last_access_unit = None;
        self.last_decoded_frame = None;
        self.note_rebuild(now);
    }

    fn age(now: f64, at: Option<f64>) -> f64 {
        match at {
            Some(when) => (now - when).max(0.0),
            None => -1.0,
        }
    }

    /// The record the watchdog reads. `live` is the session's, not this tracker's.
    pub fn snapshot(&self, now: f64, live: bool) -> OpcWatchdogSnapshot {
        OpcWatchdogSnapshot {
            now,
            last_decoded_frame_age: Self::age(now, self.last_decoded_frame),
            last_video_packet_age: Self::age(now, self.last_video_packet),
            last_access_unit_age: Self::age(now, self.last_access_unit),
            last_status_age: Self::age(now, self.last_status),
            last_ble_notify_age: -1.0,
            seconds_since_last_rebuild: Self::age(now, self.last_rebuild),
            seconds_since_last_enable: Self::age(now, self.last_enable),
            seconds_since_focus_track_set: Self::age(now, self.last_focus_track_set),
            seconds_since_zoom_set: Self::age(now, self.last_zoom_set),
            seconds_since_gimbal_throw: Self::age(now, self.last_gimbal_throw),
            seconds_since_camera_set: Self::age(now, self.last_camera_set),
            flow_healthy: i32::from(self.flow_healthy),
            path_ready: i32::from(self.path_ready),
            has_format: i32::from(self.has_format),
            decoder_failed: i32::from(self.decoder_failed),
            live: i32::from(live),
            saw_picture: i32::from(self.saw_picture),
            tcp_poke_ready: 0,
            displayed_image_removed: 0,
            had_video: i32::from(self.saw_picture),
            reserved: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_seen_reads_as_never_rather_than_now() {
        let health = FeedHealth::new();
        let snapshot = health.snapshot(100.0, true);
        // Zero is a real age. "Never" has to be distinguishable from "just now".
        assert!(snapshot.last_video_packet_age < 0.0);
        assert!(snapshot.last_access_unit_age < 0.0);
        assert!(snapshot.seconds_since_last_enable < 0.0);
        assert_eq!(snapshot.saw_picture, 0);
    }

    #[test]
    fn ages_count_forward_from_when_a_thing_happened() {
        let mut health = FeedHealth::new();
        health.note_video_packet(10.0);
        health.note_access_unit(11.0);
        let snapshot = health.snapshot(15.0, true);
        assert_eq!(snapshot.last_video_packet_age, 5.0);
        assert_eq!(snapshot.last_access_unit_age, 4.0);
        assert_eq!(snapshot.saw_picture, 1);
        assert_eq!(snapshot.has_format, 1);
    }

    #[test]
    fn an_age_never_goes_negative_when_the_clock_wobbles() {
        let mut health = FeedHealth::new();
        health.note_video_packet(10.0);
        // A monotonic clock should not go backwards, but a negative age would read as
        // "never seen" and quietly disarm the watchdog.
        assert_eq!(health.snapshot(9.0, true).last_video_packet_age, 0.0);
    }

    #[test]
    fn zoom_and_focus_track_also_count_as_camera_writes() {
        let mut health = FeedHealth::new();
        health.note_zoom(5.0);
        let snapshot = health.snapshot(6.0, true);
        assert_eq!(snapshot.seconds_since_zoom_set, 1.0);
        assert_eq!(snapshot.seconds_since_camera_set, 1.0);

        let mut health = FeedHealth::new();
        health.note_focus_track(5.0);
        let snapshot = health.snapshot(6.0, true);
        assert_eq!(snapshot.seconds_since_focus_track_set, 1.0);
        assert_eq!(snapshot.seconds_since_camera_set, 1.0);
    }

    #[test]
    fn a_rebuild_forgets_the_feed_but_not_that_there_was_one() {
        let mut health = FeedHealth::new();
        health.note_video_packet(10.0);
        health.note_access_unit(10.0);
        health.note_datalink_rebuilt(20.0);

        let snapshot = health.snapshot(21.0, true);
        assert!(snapshot.last_video_packet_age < 0.0, "the old feed is gone");
        assert_eq!(snapshot.seconds_since_last_rebuild, 1.0);
        // The watchdog treats a feed that never showed anything differently from one
        // that froze, so this has to survive the rebuild.
        assert_eq!(snapshot.saw_picture, 1);
        assert_eq!(snapshot.had_video, 1);
    }

    #[test]
    fn the_session_says_whether_it_is_live_not_the_tracker() {
        let health = FeedHealth::new();
        assert_eq!(health.snapshot(1.0, false).live, 0);
        assert_eq!(health.snapshot(1.0, true).live, 1);
    }

    #[test]
    fn path_and_flow_default_to_healthy_and_can_be_told_otherwise() {
        let mut health = FeedHealth::new();
        let snapshot = health.snapshot(1.0, true);
        assert_eq!((snapshot.path_ready, snapshot.flow_healthy), (1, 1));

        health.set_path_ready(false);
        health.set_flow_healthy(false);
        health.set_decoder_failed(true);
        let snapshot = health.snapshot(1.0, true);
        assert_eq!(snapshot.path_ready, 0);
        assert_eq!(snapshot.flow_healthy, 0);
        assert_eq!(snapshot.decoder_failed, 1);
    }
}
