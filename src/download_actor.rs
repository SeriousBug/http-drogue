use std::collections::HashMap;
use std::time::Instant;

use futures::{future, StreamExt};
use lazy_static::lazy_static;
use ractor::{
    concurrency::JoinHandle, Actor, ActorId, ActorProcessingErr, ActorRef, SupervisionEvent,
};
use regex::Regex;
use reqwest::Client;
use sanitize_filename::sanitize;
use tokio::{
    fs::{self, OpenOptions},
    io::AsyncWriteExt,
};
use tracing::{debug, error, info, warn};
use ulid::Ulid;

use crate::{
    avg_range::MovingAverage,
    store::{DownloadProgressStore, Progress},
};

pub fn url_to_filename(url: &str) -> String {
    lazy_static! {
        // Find the last segment of the URL, discarding any query parameters
        static ref RE: Regex = Regex::new(r#"/([^?/]+)([?].*)?$"#).unwrap();
    }
    RE.captures(&url)
        .and_then(|v| v.get(1))
        .map(|v| v.as_str().to_string())
        .unwrap_or_else(|| sanitize(&url))
}

#[derive(Debug)]

pub struct Coordinator {
    /// How many files to download at once. The coordinator will launch this many
    /// downloaders.
    pub concurrent_downloads: usize,

    pub store: DownloadProgressStore,
}

#[derive(Debug)]

pub struct CoordinatorState {
    pub children: HashMap<ActorId, DownloaderRef>,
}

#[derive(Debug)]
pub struct DownloaderRef {
    pub id: ActorId,
    pub url: String,
    pub actor: ActorRef<Downloader>,
    pub handle: JoinHandle<()>,
    pub retries: u64,
}

#[derive(Debug, Clone)]
pub struct StartDownload {
    pub url: String,
}

#[derive(Debug, Clone)]
pub enum CoordinatorMsg {
    StartDownload(StartDownload),
}

#[derive(Debug)]

/// An actor that downloads a file.
pub struct Downloader {
    pub url: String,
    pub coordinator: ActorRef<Coordinator>,
    pub store: DownloadProgressStore,
}

#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("Failed to download file, it was not found: {0}")]
    NotFound(String),
}

#[async_trait::async_trait]
impl Actor for Downloader {
    /// Downloader does not accept any messages, you start a download and let it finish.
    type Msg = ();

    type State = ();
    type Arguments = ();

    /// Open the file and get ready to write
    async fn pre_start(
        &self,
        _myself: ActorRef<Self>,
        _: (),
    ) -> Result<Self::State, ActorProcessingErr> {
        Ok(())
    }

    /// Start the download and send progress updates to the coordinator.
    async fn post_start(
        &self,
        myself: ActorRef<Self>,
        _state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        let filename = self
            .store
            .get(&self.url)
            .await?
            .and_then(|v| v.target_file)
            .unwrap_or_else(|| format!(".{}.tmp", Ulid::new().to_string()));
        info!("Downloading {} to {}", self.url, &filename);

        // If a file exists, resume from where it left off. We can't read the
        // progress from the store because all of the file data might not have
        // gotten persisted to the disk if there was a power outage or crash.
        let resume_progress = fs::metadata(&filename).await.map(|v| v.len()).unwrap_or(0);

        let url = self.url.clone();
        let client = Client::new();
        let mut req_builder = client.get(&url);
        if resume_progress > 0 {
            req_builder = req_builder.header("Range", format!("bytes={}-", resume_progress));
        }
        let req = req_builder.send().await?;

        if req.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(DownloadError::NotFound(url).into());
        }
        let resuming = req.status() == reqwest::StatusCode::PARTIAL_CONTENT;
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            // If resuming, don't truncate the data. If this is the first time
            // we're downloading, or if the server doesn't support resuming,
            // then truncate the file to start from the beginning.
            .truncate(!resuming)
            // If we are resuming, then we want to append to the end.
            .append(resuming)
            .open(&filename)
            .await?;

        let total = req.content_length();
        let mut progress: u64 = resume_progress;

        let mut last_update = Instant::now();
        let mut bytes_since_last_update = 0u64;

        let mut download_speed_average = MovingAverage::new();

        let mut bytes = req.bytes_stream();
        while let Some(chunk) = bytes.next().await {
            let chunk = chunk?;
            let completed = chunk.len() as u64;

            file.write_all(&chunk).await?;
            progress += completed;
            bytes_since_last_update += completed;

            // Every second or so, we send out an update of how much we've
            // downloaded, and what our current speed estimate is.
            let time_since_last_update = Instant::now().duration_since(last_update).as_millis();
            if time_since_last_update > 1000 {
                download_speed_average.add(bytes_since_last_update, time_since_last_update as u64);
                self.store
                    .put(
                        &url,
                        &Progress {
                            target_file: Some(filename.clone()),
                            failed: false,
                            url: url.clone(),
                            total,
                            progress,
                            // bytes per millisecond to bytes per second
                            speed: download_speed_average.average() / 1000.0,
                        },
                    )
                    .await?;
                last_update = Instant::now();
                bytes_since_last_update = 0;
            }
        }

        // Make sure the data is written to disk before we call the download complete
        file.flush().await?;
        file.sync_all().await?;
        drop(file);

        let final_filename = url_to_filename(&self.url);
        info!("Putting download into {}", final_filename);
        fs::rename(filename, final_filename).await?;

        myself.stop(None);
        Ok(())
    }
}

static MAX_RETRIES: u64 = 24;

impl Coordinator {
    async fn start_download(
        &self,
        myself: &ActorRef<Self>,
        state: &mut CoordinatorState,
        url: &str,
        existing_retries: u64,
    ) -> Result<(), ActorProcessingErr> {
        let downloader = Downloader {
            url: url.to_string(),
            coordinator: myself.clone(), // cloning the reference, not the actor
            store: self.store.clone(),
        };
        let (actor, handle) = Actor::spawn_linked(None, downloader, (), myself.get_cell()).await?;

        state.children.insert(
            actor.get_id(),
            DownloaderRef {
                id: actor.get_id(),
                url: url.to_string(),
                actor,
                handle,
                retries: existing_retries + 1,
            },
        );
        Ok(())
    }
}

#[async_trait::async_trait]
impl Actor for Coordinator {
    type Msg = CoordinatorMsg;
    type State = CoordinatorState;
    type Arguments = ();

    /// Open the file and get ready to write
    async fn pre_start(
        &self,
        _myself: ActorRef<Self>,
        _: (),
    ) -> Result<Self::State, ActorProcessingErr> {
        debug!("Starting coordinator");
        Ok(CoordinatorState {
            children: HashMap::new(),
        })
    }

    /// If the app is stopped while downloads are in progress, those downloads
    /// are interrupted and must be restarted or resumed. We do that here.
    async fn post_start(
        &self,
        myself: ActorRef<Self>,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        let files = self.store.scan().await.unwrap();
        let files = files.map(|x| x.unwrap().1);
        let files = files.filter(|x| future::ready(!x.failed));
        let files = files.collect::<Vec<_>>().await;

        for file in files {
            self.start_download(&myself, state, &file.url, 0).await?;
        }
        Ok(())
    }

    async fn handle(
        &self,
        myself: ActorRef<Self>,
        message: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match message {
            CoordinatorMsg::StartDownload(download) => {
                self.start_download(&myself, state, &download.url, 0)
                    .await?;
            }
        }

        Ok(())
    }

    async fn handle_supervisor_evt(
        &self,
        myself: ActorRef<Self>,
        message: SupervisionEvent,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match message {
            SupervisionEvent::ActorTerminated(child, _state, _reason) => {
                let url = state.children.get(&child.get_id()).unwrap().url.clone();
                info!("Download finished: {:?}", url);
                self.store.delete(url).await?;
                state.children.remove(&child.get_id());
                Ok(())
            }
            SupervisionEvent::ActorPanicked(child, err) => {
                let child = state.children.get(&child.get_id()).unwrap();
                let url = child.url.clone();
                let child_id = child.id.clone();
                drop(child);

                if child.retries > MAX_RETRIES {
                    error!("Download failed, giving up: {:?}", url);

                    let last_state = self
                        .store
                        .get(&url)
                        .await?
                        .unwrap_or_else(|| Progress::default_with(url.clone()));
                    // Update the state to indicate that the download failed
                    self.store
                        .put(
                            &url,
                            &Progress {
                                failed: true,
                                ..last_state
                            },
                        )
                        .await?;

                    state.children.remove(&child_id);
                    return Ok(());
                }

                warn!("Download failed, restarting: {:?}, {:?}", &url, err);

                self.start_download(&myself, state, &url, child.retries)
                    .await?;
                Ok(())
            }
            _ => Ok(()),
        }
    }
}
