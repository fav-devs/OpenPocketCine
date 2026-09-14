//! The window's side of the media browser: everything that touches a socket, a file
//! or the clock. The shell decides what the screens show; this fetches, lists, plays
//! and brings live view back.

use std::path::PathBuf;

use opc_camera::{Command, DumlFrame};
use opc_decode::{FileReader, OwnedPicture};
use opc_media::browse::BrowseConfig;
use opc_media::{
    catalog, Browse, BrowseEvent, BrowseStep, MediaCache, MediaFile, MediaJob, MediaReport,
    MediaWorker, ResumeAction, ResumePolicy,
};
use opc_monitor::{MediaAction, Shell};

/// Leaving playback: exit until the bit clears, then enable live view.
#[derive(Debug, Clone, Copy)]
struct Resume {
    attempt: u32,
    started: f64,
    last_sent: f64,
    exit_acked: bool,
}

const RESUME_PERIOD: f64 = 0.6;
const SCREEN_PICTURE: (u32, u32) = (1280, 720);

#[derive(Debug, Default)]
pub struct MediaDriver {
    cache: Option<MediaCache>,
    worker: Option<MediaWorker>,
    browse: Option<Browse>,
    resume: Option<Resume>,
    in_playback: bool,
    single_sd: bool,
    special_entry: bool,
    /// The clip on screen and the clock it runs on.
    reader: Option<FileReader>,
    playing: bool,
    /// Presentation time of the frame on screen, and the wall clock it was shown at.
    shown_ms: i64,
    shown_at: f64,
    /// The next frame, decoded ahead of its time.
    next: Option<(OwnedPicture, i64)>,
    /// A clip the operator asked to play; the worker is fetching it.
    pending_play: Option<MediaFile>,
    pending_photo: Option<MediaFile>,
    /// What a screen presents under the chrome: black, a still, or the clip's frame.
    picture: Option<OwnedPicture>,
    /// The last frame presented, so the resume loop can tell fresh from stale.
    last_presented: Option<f64>,
}

impl MediaDriver {
    /// Which body this is decides the store rule and the playback entry.
    pub fn set_body(&mut self, model_id: Option<i32>) {
        // Osmo Pocket 3 is `0x0020`: one microSD, and `0x02/0x0c` refused after a take.
        self.single_sd = model_id == Some(0x20);
        self.special_entry = true;
        if let Some(worker) = &self.worker {
            worker.ask(MediaJob::SingleSd(self.single_sd));
        }
    }

    pub fn is_active(&self) -> bool {
        self.browse.is_some() || self.resume.is_some() || self.reader.is_some()
    }

    /// The picture a non-viewfinder screen presents.
    pub fn picture(&mut self) -> &OwnedPicture {
        self.picture
            .get_or_insert_with(|| OwnedPicture::black(SCREEN_PICTURE.0, SCREEN_PICTURE.1))
    }

    pub fn note_presented(&mut self, now: f64) {
        self.last_presented = Some(now);
    }

    fn ensure_worker(&mut self, camera_id: &str) {
        if self.worker.is_none() {
            let cache = MediaCache::for_camera(camera_id);
            self.worker = Some(MediaWorker::spawn(cache.clone(), self.single_sd));
            self.cache = Some(cache);
        }
    }

    /// A camera reply the browser reads.
    pub fn frame(&mut self, frame: DumlFrame, now: f64) {
        if (frame.cmd_set, frame.cmd_id) == (0x02, 0x0C) {
            if let Some(resume) = self.resume.as_mut() {
                resume.exit_acked = true;
            }
        }
        if let Some(browse) = self.browse.as_mut() {
            if (frame.cmd_set, frame.cmd_id) == (0x00, 0x27) {
                browse.note_chunk(now);
            }
            browse.event(BrowseEvent::Frame(frame));
        }
    }

    pub fn status(&mut self, in_playback: bool) {
        self.in_playback = in_playback;
        if let Some(browse) = self.browse.as_mut() {
            browse.event(BrowseEvent::InPlayback(in_playback));
        }
    }

    /// Carries out what the shell asked. Returns the commands to put on the wire.
    pub fn action(
        &mut self,
        shell: &mut Shell,
        action: MediaAction,
        camera_id: &str,
        now: f64,
    ) -> Vec<Command> {
        match action {
            MediaAction::OpenLibrary => {
                self.ensure_worker(camera_id);
                self.resume = None;
                if let Some(cache) = &self.cache {
                    let library = shell.library_mut();
                    if library.files.is_empty() {
                        library.files = cache.load_index();
                    }
                    library.favorites = cache.load_favorites();
                    for file in library.files.clone() {
                        if cache.has_original(&file) {
                            library.cached.insert(file.path.clone());
                        }
                        if cache.cached_proxy(&file).is_some() {
                            library.proxies.insert(file.path.clone());
                        }
                    }
                }
                self.start_browse(now);
            }
            MediaAction::Refresh => {
                self.start_browse(now);
            }
            MediaAction::CloseLibrary => {
                self.browse = None;
                self.stop_player();
                self.picture = None;
                self.save(shell);
                self.resume = Some(Resume {
                    attempt: 0,
                    started: now,
                    last_sent: now - RESUME_PERIOD,
                    exit_acked: false,
                });
            }
            MediaAction::Thumb(file) => {
                if let Some(worker) = &self.worker {
                    worker.ask(MediaJob::Thumb(file));
                }
            }
            MediaAction::Download(file) => {
                if let Some(worker) = &self.worker {
                    worker.ask(MediaJob::Original(file));
                }
            }
            MediaAction::Play(file) => {
                self.pending_play = Some(file.clone());
                shell.library_progress(&file.path, 0, None);
                if let Some(worker) = &self.worker {
                    worker.ask(MediaJob::Proxy(file));
                }
            }
            MediaAction::Photo(file) => {
                self.pending_photo = Some(file.clone());
                shell.library_progress(&file.path, 0, None);
                if let Some(worker) = &self.worker {
                    worker.ask(MediaJob::Photo(file));
                }
            }
            MediaAction::PlayerToggle => {
                self.playing = shell.player().is_some_and(|player| player.playing);
                self.shown_at = now;
                if self.playing && self.reader.is_some() {
                    // Restarting at the end plays the clip again.
                    if let Some(player) = shell.player() {
                        if player.position_ms >= player.duration_ms && player.duration_ms > 0 {
                            self.seek(shell, 0);
                        }
                    }
                }
            }
            MediaAction::PlayerSeek(position_ms) => {
                self.seek(shell, position_ms);
                self.shown_at = now;
            }
            MediaAction::ClosePlayer => {
                self.stop_player();
                self.picture = None;
            }
        }
        Vec::new()
    }

    fn start_browse(&mut self, now: f64) {
        self.browse = Some(Browse::start(
            BrowseConfig {
                single_sd: self.single_sd,
                special_entry: self.special_entry,
            },
            now,
        ));
    }

    fn save(&self, shell: &Shell) {
        if let Some(cache) = &self.cache {
            let library = shell.library();
            let _ = cache.save_index(&library.files);
            let _ = cache.save_favorites(&library.favorites);
        }
    }

    fn stop_player(&mut self) {
        self.reader = None;
        self.next = None;
        self.playing = false;
        self.pending_play = None;
        self.pending_photo = None;
    }

    fn seek(&mut self, shell: &mut Shell, position_ms: i64) {
        let Some(reader) = self.reader.as_mut() else {
            return;
        };
        if reader.seek(position_ms).is_ok() {
            self.next = None;
            // Decode up to the asked frame so a scrub lands where the thumb is.
            let mut landed = None;
            while let Ok(Some((picture, pts))) = reader.next_picture() {
                let past = pts >= position_ms;
                landed = Some((picture, pts));
                if past {
                    break;
                }
            }
            if let Some((picture, pts)) = landed {
                self.picture = Some(picture);
                self.shown_ms = pts;
                shell.player_position(pts);
            }
        }
    }

    fn open_clip(
        &mut self,
        shell: &mut Shell,
        file: MediaFile,
        local: PathBuf,
        proxy: bool,
        now: f64,
    ) {
        match FileReader::open(&local) {
            Ok(mut reader) => {
                let info = reader.info();
                let first = reader.next_picture().ok().flatten();
                if let Some((picture, pts)) = first {
                    self.picture = Some(picture);
                    self.shown_ms = pts;
                }
                self.next = reader.next_picture().ok().flatten();
                self.reader = Some(reader);
                self.playing = true;
                self.shown_at = now;
                shell.library_file_ready(&file.path, proxy);
                shell.open_player(file, info.duration_ms, proxy, false);
            }
            Err(error) => {
                shell.library_failed(&file.path, &format!("cannot play: {error}"));
            }
        }
    }

    /// Advances everything once per frame. Returns the commands to put on the wire.
    pub fn tick(&mut self, shell: &mut Shell, now: f64) -> Vec<Command> {
        let mut commands = Vec::new();

        // Listing.
        if let Some(browse) = self.browse.as_mut() {
            let mut finished = false;
            for step in browse.tick(now) {
                match step {
                    BrowseStep::Send(command) => commands.push(command),
                    BrowseStep::PageReady {
                        sd,
                        internal,
                        merged,
                        ..
                    } => match catalog::decode_page(&sd, &internal, &merged) {
                        Ok(files) => {
                            let handles: Vec<u32> = files.iter().map(|f| f.handle).collect();
                            let count = files.len();
                            shell.library_listed(files, false);
                            browse.page_decoded(count, &handles, now);
                        }
                        Err(error) => {
                            shell.library_status(format!("The list did not read: {error}"));
                            finished = true;
                        }
                    },
                    BrowseStep::Done => finished = true,
                }
            }
            if finished {
                self.browse = None;
                let count = shell.library().files.len();
                if count == 0 {
                    shell.library_status(
                        "The camera listed nothing. A Pocket 3 lists only after playback opens; try Refresh.",
                    );
                } else {
                    shell.library_listed(Vec::new(), true);
                }
                self.save(shell);
            }
        }

        // Fetches.
        let reports = self
            .worker
            .as_ref()
            .map(MediaWorker::drain)
            .unwrap_or_default();
        for report in reports {
            match report {
                MediaReport::Thumb { path, picture } => {
                    shell.library_thumb(&path, picture.width, picture.height, &picture.pixels);
                }
                MediaReport::Photo { path, picture } => {
                    if let Some(file) = self.pending_photo.take().filter(|file| file.path == path) {
                        self.picture = Some(OwnedPicture::from_rgba(
                            picture.width,
                            picture.height,
                            &picture.pixels,
                        ));
                        shell.library_file_ready(&path, false);
                        shell.open_player(file, 0, false, true);
                    }
                }
                MediaReport::Progress { path, done, total } => {
                    shell.library_progress(&path, done, total);
                }
                MediaReport::Ready { path, local, proxy } => {
                    if let Some(file) = self.pending_play.take().filter(|file| file.path == path) {
                        self.open_clip(shell, file, local, proxy, now);
                    } else {
                        shell.library_file_ready(&path, proxy);
                    }
                }
                MediaReport::Failed { path, reason } => {
                    if self.pending_play.as_ref().is_some_and(|f| f.path == path) {
                        self.pending_play = None;
                    }
                    if self.pending_photo.as_ref().is_some_and(|f| f.path == path) {
                        self.pending_photo = None;
                    }
                    shell.library_failed(&path, &reason);
                }
            }
        }

        // Playback pacing: show each frame when its time comes.
        if self.playing {
            if let Some(reader) = self.reader.as_mut() {
                let target_ms = self.shown_ms + ((now - self.shown_at) * 1000.0) as i64;
                let mut ended = false;
                while let Some((_, pts)) = &self.next {
                    if *pts > target_ms {
                        break;
                    }
                    let (picture, pts) = self.next.take().unwrap();
                    self.picture = Some(picture);
                    self.shown_ms = pts;
                    self.shown_at = now;
                    shell.player_position(pts);
                    match reader.next_picture() {
                        Ok(Some(next)) => self.next = Some(next),
                        Ok(None) => {
                            ended = true;
                            break;
                        }
                        Err(_) => {
                            ended = true;
                            break;
                        }
                    }
                }
                if ended && self.next.is_none() {
                    self.playing = false;
                    shell.player_ended();
                }
            }
        }

        // Leaving playback for live view.
        if let Some(resume) = self.resume.as_mut() {
            if now - resume.last_sent >= RESUME_PERIOD {
                resume.last_sent = now;
                let fresh = ResumePolicy::is_picture_fresh(self.last_presented, resume.started);
                match ResumePolicy::action(
                    resume.attempt,
                    self.in_playback,
                    resume.exit_acked,
                    fresh,
                ) {
                    ResumeAction::ExitPlayback => {
                        resume.attempt += 1;
                        commands.push(Command::ExitPlayback);
                    }
                    ResumeAction::EnableLiveView => {
                        resume.attempt += 1;
                        commands.push(Command::LiveViewEnable);
                    }
                    ResumeAction::Done => self.resume = None,
                }
                if self.resume.is_some_and(|r| r.attempt > 24) {
                    self.resume = None;
                }
            }
        }

        commands
    }
}
