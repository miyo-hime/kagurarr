use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::instrument;

pub struct LidarrClient {
    client: Client,
    base_url: String,
    api_key: String,
}

// just the fields we actually use
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
    pub id: i64,
    pub artist_name: String,
}

#[derive(Debug, Serialize)]
struct CommandRequest<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "authorId")]
    author_id: Option<i64>,
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
        // paginated - grab up to page 1 for now, we apply max_albums_per_run elsewhere
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

        let albums: Vec<WantedAlbum> = serde_json::from_value(serde_json::Value::Array(records.clone()))
            .context("couldn't deserialize wanted albums")?;

        Ok(albums)
    }

    #[instrument(skip(self), fields(path = %download_path))]
    pub async fn trigger_import(&self, download_path: &str) -> Result<()> {
        self.client
            .post(self.url("/api/v1/command"))
            .header("X-API-Key", &self.api_key)
            .json(&CommandRequest {
                name: "DownloadedAlbumsScan",
                path: Some(download_path),
                author_id: None,
            })
            .send()
            .await
            .context("lidarr import command failed")?
            .error_for_status()
            .context("lidarr returned an error on import")?;

        Ok(())
    }
}
