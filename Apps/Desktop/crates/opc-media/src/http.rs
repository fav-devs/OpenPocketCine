//! The camera's SoftAP HTTP server: `/v2?storage=N&path=…` on port 80.
//!
//! Every fetch tries the store the shell resolved first and the other mount second,
//! and remembers which one answered, the way `CameraMedia` does on iOS. A Pocket 3 is
//! a single card and is never asked for `storage=1`.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

use crate::model::{self, MediaFile};

/// Which store last answered for each path, plus the single-card rule.
#[derive(Debug, Clone, Default)]
pub struct StorageMemo {
    pub single_sd: bool,
    winners: HashMap<String, i64>,
}

impl StorageMemo {
    pub fn new(single_sd: bool) -> Self {
        Self {
            single_sd,
            winners: HashMap::new(),
        }
    }

    pub fn remember(&mut self, path: &str, storage: i64) {
        self.winners.insert(path.to_string(), storage);
    }

    pub fn winner(&self, path: &str) -> Option<i64> {
        self.winners.get(path).copied()
    }

    /// The stores to try for a file, first choice first.
    pub fn order(&self, file: &MediaFile, path: &str) -> Vec<i64> {
        let first = model::resolved_storage(
            file.storage,
            file.favorite_handle(),
            self.winner(path),
            self.single_sd,
        );
        if self.single_sd {
            vec![0]
        } else if first == 0 {
            vec![0, 1]
        } else {
            vec![1, 0]
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpError(pub String);

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for HttpError {}

/// A blocking client sized for the SoftAP: short to first byte, long for a transfer.
#[derive(Debug, Clone)]
pub struct MediaHttp {
    agent: ureq::Agent,
}

impl Default for MediaHttp {
    fn default() -> Self {
        Self::new()
    }
}

impl MediaHttp {
    pub fn new() -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(5))
            .timeout_read(Duration::from_secs(30))
            .timeout_write(Duration::from_secs(30))
            .build();
        Self { agent }
    }

    /// GETs a whole asset from one store.
    pub fn get(&self, storage: i64, path: &str) -> Result<Vec<u8>, HttpError> {
        let response = self
            .agent
            .get(&model::path_url(storage, path))
            .set("Accept", "*/*")
            .call()
            .map_err(|error| HttpError(error.to_string()))?;
        let mut bytes = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut bytes)
            .map_err(|error| HttpError(error.to_string()))?;
        Ok(bytes)
    }

    /// GETs `bytes=<from>-<to>` inclusive.
    pub fn get_range(
        &self,
        storage: i64,
        path: &str,
        from: u64,
        to: u64,
    ) -> Result<Vec<u8>, HttpError> {
        let response = self
            .agent
            .get(&model::path_url(storage, path))
            .set("Accept", "*/*")
            .set("Range", &format!("bytes={from}-{to}"))
            .call()
            .map_err(|error| HttpError(error.to_string()))?;
        let mut bytes = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut bytes)
            .map_err(|error| HttpError(error.to_string()))?;
        Ok(bytes)
    }

    /// GETs a small asset, trying the stores in the memo's order and remembering the
    /// one that answered.
    pub fn fetch(
        &self,
        memo: &mut StorageMemo,
        file: &MediaFile,
        path: &str,
    ) -> Result<Vec<u8>, HttpError> {
        let mut last = HttpError("no store answered".to_string());
        for storage in memo.order(file, path) {
            match self.get(storage, path) {
                Ok(bytes) => {
                    memo.remember(path, storage);
                    return Ok(bytes);
                }
                Err(error) => last = error,
            }
        }
        Err(last)
    }

    /// Streams an asset to disk, reporting `(done, total)` as it goes. Writes to a
    /// `.part` file and renames on completion so a cut transfer never looks whole.
    pub fn download(
        &self,
        memo: &mut StorageMemo,
        file: &MediaFile,
        path: &str,
        destination: &Path,
        mut progress: impl FnMut(u64, Option<u64>),
    ) -> Result<(), HttpError> {
        let mut last = HttpError("no store answered".to_string());
        for storage in memo.order(file, path) {
            let response = match self
                .agent
                .get(&model::path_url(storage, path))
                .set("Accept", "*/*")
                .call()
            {
                Ok(response) => response,
                Err(error) => {
                    last = HttpError(error.to_string());
                    continue;
                }
            };
            let total = response
                .header("Content-Length")
                .and_then(|value| value.parse::<u64>().ok());
            let part = destination.with_extension("part");
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent).map_err(|e| HttpError(e.to_string()))?;
            }
            let mut out = File::create(&part).map_err(|e| HttpError(e.to_string()))?;
            let mut reader = response.into_reader();
            let mut buffer = vec![0u8; 256 * 1024];
            let mut done = 0u64;
            let copied = loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break Ok(()),
                    Ok(n) => {
                        if let Err(error) = out.write_all(&buffer[..n]) {
                            break Err(HttpError(error.to_string()));
                        }
                        done += n as u64;
                        progress(done, total);
                    }
                    Err(error) => break Err(HttpError(error.to_string())),
                }
            };
            drop(out);
            match copied {
                Ok(()) if total.is_none_or(|t| t == done) => {
                    std::fs::rename(&part, destination).map_err(|e| HttpError(e.to_string()))?;
                    memo.remember(path, storage);
                    return Ok(());
                }
                Ok(()) => {
                    let _ = std::fs::remove_file(&part);
                    last = HttpError(format!(
                        "transfer cut at {done} of {} bytes",
                        total.unwrap_or(0)
                    ));
                }
                Err(error) => {
                    let _ = std::fs::remove_file(&part);
                    last = error;
                }
            }
        }
        Err(last)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(storage: i64) -> MediaFile {
        MediaFile {
            path: "DCIM/DJI_001/DJI_20260814125250_0034_D.MP4".to_string(),
            handle: 0x4010_4480,
            storage,
            ..MediaFile::default()
        }
    }

    #[test]
    fn the_memo_orders_stores_by_winner_then_stamp() {
        let mut memo = StorageMemo::new(false);
        assert_eq!(memo.order(&clip(1), "a"), [1, 0]);
        memo.remember("a", 0);
        assert_eq!(memo.order(&clip(1), "a"), [0, 1]);
        assert_eq!(memo.order(&clip(0), "b"), [0, 1]);
    }

    #[test]
    fn a_single_card_body_is_asked_only_for_store_zero() {
        let mut memo = StorageMemo::new(true);
        memo.remember("a", 1);
        assert_eq!(memo.order(&clip(1), "a"), [0]);
    }
}
