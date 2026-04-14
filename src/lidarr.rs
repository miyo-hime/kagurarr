use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{info, instrument};

pub struct LidarrClient {
    client: Client,
    base_url: String,
    api_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WantedAlbum {
    pub id: i64,
    pub title: String,
    pub artist: Artist,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artist {
    pub artist_name: String,
}

#[derive(Debug)]
pub enum ImportResult {
    Accepted,
    Rejected(String),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommandResponse {
    id: i64,
    status: String,
    message: Option<String>,
}

#[derive(Debug, Serialize)]
struct CommandRequest<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<&'a str>,
}

impl LidarrClient {
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

    #[instrument(skip(self))]
    pub async fn wanted_albums(&self) -> Result<Vec<WantedAlbum>> {
        let resp: serde_json::Value = self
            .client
            .get(self.url("/api/v1/wanted/missing"))
            .header("X-API-Key", &self.api_key)
            .query(&[("pageSize", "50"), ("page", "1")])
            .send()
            .await
            .context("lidarr wanted/missing request failed")?
            .error_for_status()
            .context("lidarr returned an error")?
            .json()
            .await
            .context("couldn't parse lidarr response")?;

        let records = resp["records"]
            .as_array()
            .context("lidarr response missing 'records'")?;

        let albums: Vec<WantedAlbum> =
            serde_json::from_value(serde_json::Value::Array(records.clone()))
                .context("couldn't deserialize wanted albums")?;

        Ok(albums)
    }

    /// tell lidarr there are files to import. returns a command id so we can check if it liked them.
    #[instrument(skip(self), fields(path = %download_path))]
    pub async fn trigger_import(&self, download_path: &str) -> Result<i64> {
        let resp: CommandResponse = self
            .client
            .post(self.url("/api/v1/command"))
            .header("X-API-Key", &self.api_key)
            .json(&CommandRequest {
                name: "DownloadedAlbumsScan",
                path: Some(download_path),
            })
            .send()
            .await
            .context("lidarr import command failed")?
            .error_for_status()
            .context("lidarr returned an error on import")?
            .json()
            .await
            .context("couldn't parse command response")?;

        Ok(resp.id)
    }

    /// wait for lidarr to finish processing, then figure out if it was happy about it.
    #[instrument(skip(self), fields(command_id = %command_id))]
    pub async fn poll_command(&self, command_id: i64) -> Result<ImportResult> {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;

            let resp: CommandResponse = self
                .client
                .get(self.url(&format!("/api/v1/command/{command_id}")))
                .header("X-API-Key", &self.api_key)
                .send()
                .await
                .context("lidarr command poll failed")?
                .error_for_status()
                .context("lidarr returned an error polling command")?
                .json()
                .await
                .context("couldn't parse command poll response")?;

            match resp.status.as_str() {
                "completed" | "failed" => {
                    let message = resp.message.unwrap_or_default();
                    // fun fact: lidarr uses status="completed" for both success AND rejection.
                    // the actual outcome is buried in the message text. yes, really.
                    // log it regardless so we can see what lidarr actually said.
                    info!("lidarr command {} finished: status={:?} message={:?}", resp.id, resp.status, message);
                    // we string-match on "failed"/"unable" because that's what the api gives us.
                    if message.to_lowercase().contains("failed")
                        || message.to_lowercase().contains("unable")
                        || message.is_empty()
                    {
                        return Ok(ImportResult::Rejected(message));
                    }
                    return Ok(ImportResult::Accepted);
                }
                _ => {} // still thinking about it
            }
        }
    }
}
