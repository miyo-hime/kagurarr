use anyhow::{bail, Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::Instant;
use tracing::instrument;
use uuid::Uuid;

pub struct SlskdClient {
    client: Client,
    base_url: String,
    api_key: String,
}

#[derive(Debug, Deserialize)]
pub struct SearchResponse {
    pub username: String,
    pub files: Vec<SlskdFile>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SlskdFile {
    pub filename: String,
    pub size: u64,
    pub extension: Option<String>,
}

#[derive(Debug, Serialize)]
struct SearchRequest<'a> {
    id: String,
    #[serde(rename = "searchText")]
    search_text: &'a str,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchStatus {
    is_complete: bool,
}

#[derive(Debug, Serialize)]
struct DownloadRequest<'a> {
    username: &'a str,
    files: Vec<DownloadFile<'a>>,
}

#[derive(Debug, Serialize)]
struct DownloadFile<'a> {
    filename: &'a str,
    size: u64,
}

// shapes for the transfer poll response - slskd nests this as users -> directories -> files
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TransferUser {
    username: String,
    directories: Vec<TransferDirectory>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TransferDirectory {
    files: Vec<TransferFile>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TransferFile {
    filename: String,
    state: String,
    bytes_transferred: u64,
    size: u64,
}

impl SlskdClient {
    pub fn new(base_url: &str, api_key: &str) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    /// search and return all responses once the search completes
    #[instrument(skip(self), fields(query = %query))]
    pub async fn search(&self, query: &str) -> Result<Vec<SearchResponse>> {
        let id = Uuid::new_v4().to_string();

        self.client
            .post(self.url("/api/v0/searches"))
            .header("X-API-Key", &self.api_key)
            .json(&SearchRequest { id: id.clone(), search_text: query })
            .send()
            .await
            .context("slskd search request failed")?
            .error_for_status()
            .context("slskd returned an error starting search")?;

        // poll until done (timeout after ~60s)
        for _ in 0..30 {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;

            let status: SearchStatus = self
                .client
                .get(self.url(&format!("/api/v0/searches/{id}")))
                .header("X-API-Key", &self.api_key)
                .send()
                .await
                .context("slskd search status poll failed")?
                .error_for_status()
                .context("slskd returned an error polling search")?
                .json()
                .await
                .context("couldn't parse search status")?;

            if status.is_complete {
                break;
            }
        }

        let responses: Vec<SearchResponse> = self
            .client
            .get(self.url(&format!("/api/v0/searches/{id}/responses")))
            .header("X-API-Key", &self.api_key)
            .send()
            .await
            .context("slskd search responses request failed")?
            .error_for_status()
            .context("slskd returned an error fetching results")?
            .json()
            .await
            .context("couldn't parse search responses")?;

        Ok(responses)
    }

    /// queue files from a single user for download
    #[instrument(skip(self, files), fields(user = %username))]
    pub async fn download(&self, username: &str, files: &[SlskdFile]) -> Result<()> {
        if files.is_empty() {
            bail!("no files to download");
        }

        let download_files: Vec<DownloadFile> = files
            .iter()
            .map(|f| DownloadFile { filename: &f.filename, size: f.size })
            .collect();

        self.client
            .post(self.url("/api/v0/transfers/downloads"))
            .header("X-API-Key", &self.api_key)
            .json(&DownloadRequest { username, files: download_files })
            .send()
            .await
            .context("slskd download request failed")?
            .error_for_status()
            .context("slskd returned an error queueing downloads")?;

        Ok(())
    }

    /// poll transfers until all our files complete, stall, or error.
    /// returns the local folder path to hand to lidarr for import.
    ///
    /// slskd downloads to {download_dir}/{username}/{remote_folder_name}/
    #[instrument(skip(self, files), fields(user = %username))]
    pub async fn poll_until_done(
        &self,
        username: &str,
        files: &[SlskdFile],
        download_dir: &str,
        stall_timeout_secs: u64,
    ) -> Result<String> {
        // track by filename so we can find our specific transfers in the global queue
        let our_files: HashSet<&str> = files.iter().map(|f| f.filename.as_str()).collect();

        let mut last_bytes: u64 = 0;
        let mut last_progress_at = Instant::now();

        loop {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;

            let all_users: Vec<TransferUser> = self
                .client
                .get(self.url("/api/v0/transfers/downloads"))
                .header("X-API-Key", &self.api_key)
                .send()
                .await
                .context("slskd transfer poll failed")?
                .error_for_status()
                .context("slskd returned an error polling transfers")?
                .json()
                .await
                .context("couldn't parse transfer response")?;

            let our_transfers: Vec<&TransferFile> = all_users
                .iter()
                .filter(|u| u.username == username)
                .flat_map(|u| u.directories.iter())
                .flat_map(|d| d.files.iter())
                .filter(|f| our_files.contains(f.filename.as_str()))
                .collect();

            if our_transfers.is_empty() {
                bail!("transfers vanished from the queue - slskd probably ate them");
            }

            let total_bytes: u64 = our_transfers.iter().map(|f| f.bytes_transferred).sum();
            let total_size: u64 = our_transfers.iter().map(|f| f.size).sum();

            // check for terminal error states first
            for f in &our_transfers {
                match f.state.as_str() {
                    "Errored" | "Cancelled" | "Rejected" | "TimedOut" => {
                        bail!("transfer hit terminal state '{}' for {}", f.state, f.filename);
                    }
                    _ => {}
                }
            }

            // all done?
            let all_complete = our_transfers.iter().all(|f| f.state == "Completed");
            if all_complete {
                let local_path = derive_local_path(download_dir, username, files);
                tracing::info!("download complete ({total_bytes} bytes), import path: {local_path}");
                return Ok(local_path);
            }

            // stall detection
            if total_bytes > last_bytes {
                last_bytes = total_bytes;
                last_progress_at = Instant::now();
                tracing::debug!("progress: {total_bytes}/{total_size} bytes");
            } else if last_progress_at.elapsed().as_secs() > stall_timeout_secs {
                bail!(
                    "download stalled - no progress for {}s ({total_bytes}/{total_size} bytes transferred)",
                    stall_timeout_secs
                );
            }
        }
    }
}

/// derive the local folder path where slskd put the files.
/// slskd mirrors: {download_dir}/{username}/{remote_folder_last_component}/
fn derive_local_path(download_dir: &str, username: &str, files: &[SlskdFile]) -> String {
    // grab the folder name from the first file's remote path
    // remote paths are backslash-separated: "\\server\Artist - Album\01 track.flac"
    let folder_name = files
        .first()
        .and_then(|f| {
            let parts: Vec<&str> = f.filename.split('\\').collect();
            // second-to-last component is the folder name
            parts.get(parts.len().saturating_sub(2)).copied()
        })
        .unwrap_or("unknown");

    format!("{}/{}/{}", download_dir.trim_end_matches('/'), username, folder_name)
}
