//! The real network: `soulseek-rs-lib`'s client behind [`Network`].

use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use soulseek_rs::{Client, ClientSettings, DownloadStatus, SearchResult};

use super::{Credentials, Format, Hit, Network, Progress, Quality};

/// A SoulSeek session, made when first needed and again after it drops.
pub struct Live {
    credentials: Credentials,
    client: Option<Client>,
    /// Peer responses already read for the search under way.
    responses: usize,
}

impl Live {
    pub fn new(credentials: Credentials) -> Self {
        Self {
            credentials,
            client: None,
            responses: 0,
        }
    }

    fn client(&self) -> Result<&Client, String> {
        self.client.as_ref().ok_or_else(|| "not logged in".into())
    }
}

impl Network for Live {
    fn connect(&mut self) -> Result<(), String> {
        if self
            .client
            .as_ref()
            .is_some_and(|c| c.session_loss().is_none())
        {
            return Ok(());
        }
        self.client = None;
        let settings = ClientSettings::new(&self.credentials.user, &self.credentials.password);
        let mut client = Client::with_settings(settings);
        client.connect().map_err(|e| e.to_string())?;
        match client.login() {
            Ok(true) => {
                self.client = Some(client);
                Ok(())
            }
            Ok(false) => Err("SoulSeek refused the login".into()),
            Err(e) => Err(e.to_string()),
        }
    }

    fn search(&mut self, query: &str) -> Result<(), String> {
        self.responses = 0;
        // No window here: the search goes out and its hits gather in the client.
        self.client()?
            .search_with_cancel(query, Duration::ZERO, None)
            .map(drop)
            .map_err(|e| e.to_string())
    }

    fn hits(&mut self, query: &str) -> Option<Vec<Hit>> {
        let client = self.client.as_ref()?;
        let responses = client.get_search_results_count(query);
        if responses == self.responses {
            return None;
        }
        self.responses = responses;
        Some(
            client
                .get_search_results(query)
                .iter()
                .flat_map(hits)
                .collect(),
        )
    }

    fn forget(&mut self, query: &str) {
        if let Some(c) = &self.client {
            let _ = c.forget_search(query);
        }
    }

    fn download(&mut self, hit: &Hit, dir: &Path) -> Result<Receiver<Progress>, String> {
        let dir = dir
            .to_str()
            .ok_or("the downloads folder's path is not UTF-8")?
            .to_string();
        let (_, status) = self
            .client()?
            .download(hit.path.clone(), hit.user.clone(), hit.size, dir)
            .map_err(|e| e.to_string())?;
        // The client's statuses, as ours, on a thread that ends with the transfer.
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("soulseek-transfer".into())
            .spawn(move || {
                for s in status {
                    let end = s.is_terminal();
                    if tx.send(progress(s)).is_err() || end {
                        return;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(rx)
    }
}

/// A peer's response as hits: its audio files, each with its quality.
fn hits(r: &SearchResult) -> impl Iterator<Item = Hit> + '_ {
    r.files.iter().filter_map(|f| {
        let format = Format::of(&f.name)?;
        Some(Hit {
            user: f.username.clone(),
            path: f.name.clone(),
            size: f.size,
            quality: Quality::new(format, &f.attribs),
            free_slot: r.slots > 0,
            speed: r.speed,
        })
    })
}

fn progress(s: DownloadStatus) -> Progress {
    match s {
        DownloadStatus::Queued => Progress::Queued,
        DownloadStatus::InProgress {
            bytes_downloaded,
            total_bytes,
            ..
        }
        | DownloadStatus::Paused {
            bytes_downloaded,
            total_bytes,
        } => Progress::Receiving {
            done: bytes_downloaded,
            total: total_bytes,
        },
        DownloadStatus::Completed => Progress::Done,
        DownloadStatus::Cancelled => Progress::Failed("cancelled".into()),
        DownloadStatus::Failed(why) => {
            Progress::Failed(why.unwrap_or_else(|| "the peer stopped sending".into()))
        }
        DownloadStatus::TimedOut => Progress::Failed("timed out".into()),
    }
}
