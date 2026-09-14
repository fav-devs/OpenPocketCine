//! A thread that fetches from the camera so the window never waits on the SoftAP.

use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::JoinHandle;

use crate::cache::MediaCache;
use crate::http::{MediaHttp, StorageMemo};
use crate::model::MediaFile;
use crate::thumbs::{self, Rgba};

/// What the window asks for.
#[derive(Debug, Clone)]
pub enum MediaJob {
    /// The `.scr` thumbnail, decoded for the grid. Served from the cache when present.
    Thumb(MediaFile),
    /// The 720p proxy to `play/`, for the player. Falls back to the original.
    Proxy(MediaFile),
    /// The original to `files/`.
    Original(MediaFile),
    /// A still, decoded for the viewer. Served from `files/` when cached.
    Photo(MediaFile),
    /// A body change: which store rule applies.
    SingleSd(bool),
}

/// What comes back.
#[derive(Debug, Clone)]
pub enum MediaReport {
    Thumb {
        path: String,
        picture: Rgba,
    },
    Photo {
        path: String,
        picture: Rgba,
    },
    Progress {
        path: String,
        done: u64,
        total: Option<u64>,
    },
    /// A file is on disk: the camera path, the local file, and whether it is a proxy.
    Ready {
        path: String,
        local: std::path::PathBuf,
        proxy: bool,
    },
    Failed {
        path: String,
        reason: String,
    },
}

#[derive(Debug)]
pub struct MediaWorker {
    jobs: Sender<MediaJob>,
    reports: Receiver<MediaReport>,
    thread: Option<JoinHandle<()>>,
}

impl MediaWorker {
    pub fn spawn(cache: MediaCache, single_sd: bool) -> Self {
        let (jobs, job_rx) = mpsc::channel::<MediaJob>();
        let (report_tx, reports) = mpsc::channel::<MediaReport>();
        let thread = std::thread::Builder::new()
            .name("opc-media".into())
            .spawn(move || run(cache, single_sd, &job_rx, &report_tx))
            .ok();
        Self {
            jobs,
            reports,
            thread,
        }
    }

    pub fn ask(&self, job: MediaJob) {
        let _ = self.jobs.send(job);
    }

    pub fn drain(&self) -> Vec<MediaReport> {
        let mut out = Vec::new();
        loop {
            match self.reports.try_recv() {
                Ok(report) => out.push(report),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return out,
            }
        }
    }
}

impl Drop for MediaWorker {
    fn drop(&mut self) {
        // Closing the job channel ends the loop; a transfer in flight finishes first.
        let (sender, _) = mpsc::channel();
        drop(std::mem::replace(&mut self.jobs, sender));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(
    cache: MediaCache,
    single_sd: bool,
    jobs: &Receiver<MediaJob>,
    reports: &Sender<MediaReport>,
) {
    let http = MediaHttp::new();
    let mut memo = StorageMemo::new(single_sd);
    while let Ok(job) = jobs.recv() {
        let report = match job {
            MediaJob::SingleSd(on) => {
                memo.single_sd = on;
                continue;
            }
            MediaJob::Thumb(file) => thumb(&http, &mut memo, &cache, &file),
            MediaJob::Photo(file) => photo(&http, &mut memo, &cache, &file, reports),
            MediaJob::Proxy(file) => proxy(&http, &mut memo, &cache, &file, reports),
            MediaJob::Original(file) => original(&http, &mut memo, &cache, &file, reports),
        };
        if reports.send(report).is_err() {
            return;
        }
    }
}

fn thumb(
    http: &MediaHttp,
    memo: &mut StorageMemo,
    cache: &MediaCache,
    file: &MediaFile,
) -> MediaReport {
    let bytes = match cache.read_thumb(file) {
        Some(bytes) => bytes,
        None => match http.fetch(memo, file, &file.thumb_path) {
            Ok(bytes) => {
                let _ = cache.write_thumb(file, &bytes);
                bytes
            }
            Err(error) => {
                return MediaReport::Failed {
                    path: file.path.clone(),
                    reason: error.to_string(),
                }
            }
        },
    };
    match thumbs::decode_jpeg(&bytes) {
        Ok(picture) => MediaReport::Thumb {
            path: file.path.clone(),
            picture: thumbs::fit(picture, 512),
        },
        Err(reason) => MediaReport::Failed {
            path: file.path.clone(),
            reason,
        },
    }
}

fn photo(
    http: &MediaHttp,
    memo: &mut StorageMemo,
    cache: &MediaCache,
    file: &MediaFile,
    reports: &Sender<MediaReport>,
) -> MediaReport {
    let local = cache.original_path(file);
    if !local.is_file() {
        if let Err(error) = http.download(memo, file, &file.path, &local, |done, total| {
            let _ = reports.send(MediaReport::Progress {
                path: file.path.clone(),
                done,
                total,
            });
        }) {
            return MediaReport::Failed {
                path: file.path.clone(),
                reason: error.to_string(),
            };
        }
    }
    let bytes = match std::fs::read(&local) {
        Ok(bytes) => bytes,
        Err(error) => {
            return MediaReport::Failed {
                path: file.path.clone(),
                reason: error.to_string(),
            }
        }
    };
    match thumbs::decode_jpeg(&bytes) {
        Ok(picture) => MediaReport::Photo {
            path: file.path.clone(),
            picture: thumbs::fit(picture, 2048),
        },
        Err(reason) => MediaReport::Failed {
            path: file.path.clone(),
            reason,
        },
    }
}

fn proxy(
    http: &MediaHttp,
    memo: &mut StorageMemo,
    cache: &MediaCache,
    file: &MediaFile,
    reports: &Sender<MediaReport>,
) -> MediaReport {
    if let Some(local) = cache.cached_proxy(file) {
        return MediaReport::Ready {
            path: file.path.clone(),
            local,
            proxy: true,
        };
    }
    // The listed proxy, the derived one, then the original: the phones' chain.
    let mut last = String::from("no proxy");
    for candidate in file.preview_paths() {
        let is_proxy = crate::model::is_proxy_path(&candidate);
        let local = if is_proxy {
            cache.play_path(&candidate)
        } else {
            cache.original_path(file)
        };
        if local.is_file() {
            return MediaReport::Ready {
                path: file.path.clone(),
                local,
                proxy: is_proxy,
            };
        }
        match http.download(memo, file, &candidate, &local, |done, total| {
            let _ = reports.send(MediaReport::Progress {
                path: file.path.clone(),
                done,
                total,
            });
        }) {
            Ok(()) => {
                return MediaReport::Ready {
                    path: file.path.clone(),
                    local,
                    proxy: is_proxy,
                }
            }
            Err(error) => last = error.to_string(),
        }
    }
    MediaReport::Failed {
        path: file.path.clone(),
        reason: last,
    }
}

fn original(
    http: &MediaHttp,
    memo: &mut StorageMemo,
    cache: &MediaCache,
    file: &MediaFile,
    reports: &Sender<MediaReport>,
) -> MediaReport {
    let local = cache.original_path(file);
    if local.is_file() {
        return MediaReport::Ready {
            path: file.path.clone(),
            local,
            proxy: false,
        };
    }
    match http.download(memo, file, &file.path, &local, |done, total| {
        let _ = reports.send(MediaReport::Progress {
            path: file.path.clone(),
            done,
            total,
        });
    }) {
        Ok(()) => MediaReport::Ready {
            path: file.path.clone(),
            local,
            proxy: false,
        },
        Err(error) => MediaReport::Failed {
            path: file.path.clone(),
            reason: error.to_string(),
        },
    }
}
