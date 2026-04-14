use anyhow::{bail, Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
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

    /// search slskd and return all responses once complete
    #[instrument(skip(self), fields(query = %query))]
    pub async fn search(&self, query: &str) -> Result<Vec<SearchResponse>> {
        let id = Uuid::new_v4().to_string();

        // kick off the search
        self.client
            .post(self.url("/api/v0/searches"))
            .header("X-API-Key", &self.api_key)
            .json(&SearchRequest { id: id.clone(), search_text: query })
            .send()
            .await
            .context("slskd search request failed")?
            .error_for_status()
            .context("slskd returned an error starting search")?;

        // poll until done (or timeout after ~60s)
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
}
