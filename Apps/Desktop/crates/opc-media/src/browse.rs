//! Listing the library: enter playback, hold it, page the catalogue.
//!
//! A pure state machine. The shell feeds it the clock, every DUML frame and the
//! playback bit, and carries out the commands it asks for; it never touches a socket,
//! so the whole sequence — including the Pocket 3's `0x01/0x01` entry — is pinned by
//! tests on a machine with no camera.
//!
//! The page sequence is the one the phones send: list the internal store, the
//! `4a040e10` trigger, then list the card, and collect until the camera goes quiet
//! (Osmosis MEDIA_PROTOCOL §1; `CameraMedia.queryMediaPage` on iOS).

use opc_camera::{Command, DumlFrame};

use crate::chunks::{
    self, ChunkAssembler, INTERNAL_COUNTER, NEWEST_INTERNAL, NEWEST_SD, SD_COUNTER,
};
use crate::resume::BrowsePolicy;

/// How the body is asked into playback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrowseConfig {
    /// A single-microSD body (Pocket 3): every `/v2` is `storage=0`.
    pub single_sd: bool,
    /// Fall through to the `0x01/0x01` entry when `0x02/0x0c` is refused or ignored.
    pub special_entry: bool,
}

impl Default for BrowseConfig {
    fn default() -> Self {
        Self {
            single_sd: false,
            special_entry: true,
        }
    }
}

/// What the shell must do for the browse to progress.
#[derive(Debug, Clone, PartialEq)]
pub enum BrowseStep {
    Send(Command),
    /// A page's chunks are complete: the counter-1 blob, the counter-2 blob, and every
    /// chunk merged. The shell decodes them and answers with [`Browse::page_decoded`].
    PageReady {
        sd: Vec<u8>,
        internal: Vec<u8>,
        merged: Vec<u8>,
        /// The cursor this page was asked with; the newest marker on page one.
        cursor: u32,
    },
    /// The library is listed, or as much of it as this body allows without playback.
    Done,
}

/// What the shell tells the browse.
#[derive(Debug, Clone, PartialEq)]
pub enum BrowseEvent {
    Frame(DumlFrame),
    /// The `0x02/0x80` playback bit as the body last reported it.
    InPlayback(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// `0x02/0x0c` sent; waiting on the reply and the bit.
    Entering,
    /// `0x01/0x01` step 1, then step 2, at 20 Hz.
    SpecialEntry,
    /// The store is not mounted the instant the bit sets; wait before the first query.
    Settling,
    /// Between the three frames of one page.
    Querying,
    /// Every frame sent; collecting chunks until quiet.
    Collecting,
    /// The shell is decoding the page.
    Decoding,
    Done,
}

const ENTER_TIMEOUT: f64 = 0.9;
const ENTER_ATTEMPTS: u32 = 3;
const SPECIAL_PERIOD: f64 = 0.05;
const SPECIAL_STEP1_FRAMES: u32 = 6;
const SPECIAL_TIMEOUT: f64 = 3.0;
const SETTLE: f64 = 1.7;
/// The internal list, the trigger, then the card list.
const QUERY_GAPS: [f64; 3] = [0.0, 0.8, 0.4];
const QUIET: f64 = 0.8;
const FLOOR: f64 = 0.8;
const PAGE_CAP: f64 = 8.0;
const PRESENCE_PERIOD: f64 = 1.0;

/// One listing of the library, from entering playback to the last page.
#[derive(Debug, Clone)]
pub struct Browse {
    config: BrowseConfig,
    phase: Phase,
    started: f64,
    phase_since: f64,
    entered: bool,
    in_playback: bool,
    enter_attempts: u32,
    enter_refused: bool,
    special_frames: u32,
    special_last: f64,
    cursor: u32,
    page_step: usize,
    chunks: ChunkAssembler,
    last_chunk_at: f64,
    collect_since: f64,
    presence_at: f64,
    policy: BrowsePolicy,
}

impl Browse {
    /// Starts at `now`; the first tick sends the playback entry.
    pub fn start(config: BrowseConfig, now: f64) -> Self {
        Self {
            config,
            phase: Phase::Entering,
            started: now,
            phase_since: now,
            entered: false,
            in_playback: false,
            enter_attempts: 0,
            enter_refused: false,
            special_frames: 0,
            special_last: now - SPECIAL_PERIOD,
            cursor: NEWEST_INTERNAL,
            page_step: 0,
            chunks: ChunkAssembler::new(),
            last_chunk_at: now,
            collect_since: now,
            presence_at: now,
            policy: BrowsePolicy::after_enter_playback(false),
        }
    }

    pub fn is_done(&self) -> bool {
        self.phase == Phase::Done
    }

    /// Whether the body confirmed playback, which is what older pages need.
    pub fn entered_playback(&self) -> bool {
        self.entered
    }

    pub fn policy(&self) -> BrowsePolicy {
        self.policy
    }

    pub fn event(&mut self, event: BrowseEvent) {
        match event {
            BrowseEvent::InPlayback(on) => {
                self.in_playback = on;
                if on && !self.entered {
                    self.entered = true;
                    self.policy = BrowsePolicy::after_enter_playback(true);
                }
            }
            BrowseEvent::Frame(frame) => {
                if (frame.cmd_set, frame.cmd_id) == (0x02, 0x0C) && frame.flags & 0x80 != 0 {
                    // The reply means received, not entered; `E0` means refused.
                    if frame.payload.first().is_some_and(|code| *code != 0) {
                        self.enter_refused = true;
                    }
                }
                if matches!(self.phase, Phase::Querying | Phase::Collecting) {
                    let now = self.last_chunk_at.max(self.collect_since);
                    if self.chunks.ingest(&frame) {
                        self.last_chunk_at = now;
                    }
                }
            }
        }
    }

    /// The shell decoded the page it was handed: how many records, and their handles.
    pub fn page_decoded(&mut self, record_count: usize, handles: &[u32], now: f64) {
        if self.phase != Phase::Decoding {
            return;
        }
        let next = chunks::next_cursor(handles, self.cursor);
        if self.policy.list_older_pages && chunks::has_older_page(record_count, next) {
            self.cursor = next.unwrap_or(NEWEST_INTERNAL);
            self.begin_page(now);
        } else {
            self.enter(Phase::Done, now);
        }
    }

    fn enter(&mut self, phase: Phase, now: f64) {
        self.phase = phase;
        self.phase_since = now;
    }

    fn begin_page(&mut self, now: f64) {
        self.chunks.reset();
        self.page_step = 0;
        self.collect_since = now;
        self.last_chunk_at = now;
        self.enter(Phase::Querying, now);
    }

    /// Advances the clock. Carry out every step, in order.
    pub fn tick(&mut self, now: f64) -> Vec<BrowseStep> {
        let mut steps = Vec::new();
        // Playback drops ~1 s after entry unless the app keeps announcing itself.
        if self.phase != Phase::Done && now - self.presence_at >= PRESENCE_PERIOD {
            self.presence_at = now;
            steps.push(BrowseStep::Send(Command::AppPresence));
        }
        // A phase that ends this tick hands straight on, so the first frame of a page
        // goes out the moment the settle is over rather than a tick later.
        let mut again = true;
        while std::mem::take(&mut again) {
            match self.phase {
                Phase::Entering => {
                    if self.entered {
                        self.enter(Phase::Settling, now);
                    } else if self.enter_refused && self.config.special_entry {
                        self.enter(Phase::SpecialEntry, now);
                        self.special_frames = 0;
                        again = true;
                    } else if self.enter_attempts == 0 || now - self.phase_since >= ENTER_TIMEOUT {
                        if self.enter_attempts >= ENTER_ATTEMPTS {
                            if self.config.special_entry {
                                self.enter(Phase::SpecialEntry, now);
                                self.special_frames = 0;
                                again = true;
                            } else {
                                // Newest page only; the policy already says so.
                                self.begin_page(now);
                                again = true;
                            }
                        } else {
                            self.enter_attempts += 1;
                            self.phase_since = now;
                            steps.push(BrowseStep::Send(Command::EnterPlayback));
                        }
                    }
                }
                Phase::SpecialEntry => {
                    if self.entered {
                        self.enter(Phase::Settling, now);
                    } else if now - self.phase_since >= SPECIAL_TIMEOUT {
                        self.begin_page(now);
                        again = true;
                    } else if now - self.special_last >= SPECIAL_PERIOD {
                        self.special_last = now;
                        let step = if self.special_frames < SPECIAL_STEP1_FRAMES {
                            1
                        } else {
                            2
                        };
                        self.special_frames += 1;
                        steps.push(BrowseStep::Send(Command::PlaybackSpecial(step)));
                    }
                }
                Phase::Settling => {
                    if now - self.phase_since >= SETTLE {
                        self.begin_page(now);
                        again = true;
                    }
                }
                Phase::Querying => {
                    let due =
                        self.phase_since + QUERY_GAPS[..=self.page_step.min(2)].iter().sum::<f64>();
                    if now >= due {
                        let command = match self.page_step {
                            0 => Command::MediaList {
                                counter: INTERNAL_COUNTER,
                                cursor: self.internal_cursor(),
                            },
                            1 => Command::MediaListTrigger,
                            _ => Command::MediaList {
                                counter: SD_COUNTER,
                                cursor: self.sd_cursor(),
                            },
                        };
                        steps.push(BrowseStep::Send(command));
                        self.page_step += 1;
                        if self.page_step == 3 {
                            self.collect_since = now;
                            self.last_chunk_at = now;
                            self.enter(Phase::Collecting, now);
                        }
                    }
                }
                Phase::Collecting => {
                    self.last_chunk_at = self.last_chunk_at.max(self.collect_since);
                    let quiet =
                        now - self.last_chunk_at >= QUIET && now - self.collect_since >= FLOOR;
                    let capped = now - self.collect_since >= PAGE_CAP;
                    if quiet || capped {
                        steps.push(BrowseStep::PageReady {
                            sd: self.chunks.assembled(SD_COUNTER).to_vec(),
                            internal: self.chunks.assembled(INTERNAL_COUNTER).to_vec(),
                            merged: self.chunks.assembled_merged(),
                            cursor: self.cursor,
                        });
                        self.enter(Phase::Decoding, now);
                    }
                }
                Phase::Decoding => {}
                Phase::Done => {}
            }
        }
        if self.phase == Phase::Done && !steps.contains(&BrowseStep::Done) {
            steps.push(BrowseStep::Done);
        }
        steps
    }

    /// A chunk arrived at `now`; the collector's quiet timer restarts.
    pub fn note_chunk(&mut self, now: f64) {
        self.last_chunk_at = now;
    }

    fn internal_cursor(&self) -> u32 {
        if self.cursor == NEWEST_INTERNAL {
            NEWEST_INTERNAL
        } else {
            self.cursor
        }
    }

    fn sd_cursor(&self) -> u32 {
        if self.cursor == NEWEST_INTERNAL {
            NEWEST_SD
        } else {
            self.cursor
        }
    }

    pub fn started_at(&self) -> f64 {
        self.started
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every command except the presence beat, which rides along every second.
    fn sends(steps: &[BrowseStep]) -> Vec<Command> {
        steps
            .iter()
            .filter_map(|step| match step {
                BrowseStep::Send(Command::AppPresence) => None,
                BrowseStep::Send(command) => Some(*command),
                _ => None,
            })
            .collect()
    }

    fn page(steps: &[BrowseStep]) -> Option<&BrowseStep> {
        steps
            .iter()
            .find(|step| matches!(step, BrowseStep::PageReady { .. }))
    }

    fn reply_0c(code: u8) -> DumlFrame {
        DumlFrame {
            sender: 0x01,
            receiver: 0x02,
            seq: 1,
            flags: 0xC0,
            cmd_set: 0x02,
            cmd_id: 0x0C,
            payload: vec![code],
        }
    }

    fn chunk(counter: u8, body: &[u8]) -> DumlFrame {
        let mut payload = vec![0x4A, 0x01, 0, 0, counter, 0, 0, 0, 0, 0];
        payload.extend_from_slice(body);
        DumlFrame {
            sender: 0x01,
            receiver: 0x02,
            seq: 1,
            flags: 0xC0,
            cmd_set: 0x00,
            cmd_id: 0x27,
            payload,
        }
    }

    #[test]
    fn a_body_that_enters_playback_settles_then_lists_the_three_frames() {
        let mut browse = Browse::start(BrowseConfig::default(), 0.0);
        assert_eq!(sends(&browse.tick(0.0)), [Command::EnterPlayback]);
        browse.event(BrowseEvent::Frame(reply_0c(0)));
        browse.event(BrowseEvent::InPlayback(true));
        assert!(
            sends(&browse.tick(0.3)).is_empty(),
            "settling, not querying yet"
        );
        assert!(sends(&browse.tick(1.5)).is_empty());
        let first = sends(&browse.tick(2.1));
        assert!(first.contains(&Command::MediaList {
            counter: 2,
            cursor: NEWEST_INTERNAL
        }));
        assert_eq!(sends(&browse.tick(2.5)), []);
        assert_eq!(sends(&browse.tick(2.95)), [Command::MediaListTrigger]);
        assert_eq!(
            sends(&browse.tick(3.4)),
            [Command::MediaList {
                counter: 1,
                cursor: NEWEST_SD
            }]
        );
        browse.event(BrowseEvent::Frame(chunk(1, b"page")));
        browse.note_chunk(3.5);
        assert!(page(&browse.tick(3.9)).is_none(), "still collecting");
        let steps = browse.tick(4.4);
        match page(&steps).expect("a page") {
            BrowseStep::PageReady { sd, cursor, .. } => {
                assert_eq!(sd, b"page");
                assert_eq!(*cursor, NEWEST_INTERNAL);
            }
            other => panic!("expected a page, got {other:?}"),
        }
        browse.page_decoded(3, &[0x4010_4480], 4.4);
        assert!(browse.is_done());
    }

    #[test]
    fn a_pocket_3_refusal_falls_through_to_the_special_entry() {
        let mut browse = Browse::start(
            BrowseConfig {
                single_sd: true,
                special_entry: true,
            },
            0.0,
        );
        browse.tick(0.0);
        browse.event(BrowseEvent::Frame(reply_0c(0xE0)));
        let mut sent = Vec::new();
        let mut t = 0.05;
        while t < 0.7 {
            sent.extend(sends(&browse.tick(t)));
            t += 0.05;
        }
        let specials: Vec<u8> = sent
            .iter()
            .filter_map(|c| match c {
                Command::PlaybackSpecial(step) => Some(*step),
                _ => None,
            })
            .collect();
        assert_eq!(&specials[..6], &[1, 1, 1, 1, 1, 1]);
        assert!(specials[6..].iter().all(|step| *step == 2));
        assert!(
            !sent.contains(&Command::EnterPlayback),
            "never re-sent 0x02/0x0c"
        );
        browse.event(BrowseEvent::InPlayback(true));
        assert!(browse.entered_playback());
        assert!(browse.policy().list_older_pages);
    }

    #[test]
    fn a_body_that_never_enters_still_lists_the_newest_page() {
        let mut browse = Browse::start(
            BrowseConfig {
                single_sd: false,
                special_entry: false,
            },
            0.0,
        );
        let mut all = Vec::new();
        let mut t = 0.0;
        while t <= 4.5 {
            all.extend(sends(&browse.tick(t)));
            t += 0.1;
        }
        let entries = all.iter().filter(|c| **c == Command::EnterPlayback).count();
        assert_eq!(entries, ENTER_ATTEMPTS as usize);
        assert!(all.iter().any(|c| matches!(c, Command::MediaList { .. })));
        assert!(!browse.policy().list_older_pages);
        let steps = browse.tick(14.0);
        assert!(page(&steps).is_some());
        browse.page_decoded(45, &[0x4010_4400], 14.0);
        assert!(
            browse.is_done(),
            "older pages need playback the body refused"
        );
    }

    #[test]
    fn older_pages_walk_the_cursor_down_while_playback_holds() {
        let mut browse = Browse::start(BrowseConfig::default(), 0.0);
        browse.tick(0.0);
        browse.event(BrowseEvent::InPlayback(true));
        browse.tick(0.1);
        browse.tick(1.9);
        browse.tick(2.8);
        browse.tick(3.2);
        let steps = browse.tick(12.0);
        assert!(page(&steps).is_some());
        browse.page_decoded(45, &[0x4010_4480, 0x4010_4400], 12.0);
        assert!(!browse.is_done());
        let next = sends(&browse.tick(12.0));
        assert!(next.contains(&Command::MediaList {
            counter: 2,
            cursor: 0x4010_4400
        }));
    }

    #[test]
    fn presence_is_announced_every_second_while_browsing() {
        let mut browse = Browse::start(BrowseConfig::default(), 0.0);
        browse.tick(0.0);
        let beats = |steps: &[BrowseStep]| {
            steps
                .iter()
                .filter(|s| **s == BrowseStep::Send(Command::AppPresence))
                .count()
        };
        assert_eq!(beats(&browse.tick(0.5)), 0);
        assert_eq!(beats(&browse.tick(1.05)), 1);
    }
}
