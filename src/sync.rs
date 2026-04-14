use anyhow::Result;
use lofty::config::WriteOptions;
use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::tag::{ItemKey, ItemValue, Tag, TagItem};
use tracing::{info, instrument, warn};

use crate::blacklist::{Blacklist, BlacklistStatus};
use crate::config::Config;
use crate::lidarr::{ImportResult, LidarrClient, WantedAlbum};
use crate::scorer::{rank_candidates, Candidate};
use crate::slskd::SlskdClient;

#[instrument(skip_all)]
pub async fn run_cycle(
    cfg: &Config,
    db: &Blacklist,
    lidarr: &LidarrClient,
    slskd: &SlskdClient,
) -> Result<()> {
    // clean up old failures so we retry them eventually
    let pruned = db.prune_expired(cfg.kagurarr.blacklist_ttl_days)?;
    if pruned > 0 {
        info!("pruned {pruned} expired blacklist entries");
    }

    let mut wanted = lidarr.wanted_albums().await?;

    // skip albums we've already handled
    wanted.retain(|a| !db.is_done(a.id).unwrap_or(false));
    wanted.truncate(cfg.kagurarr.max_albums_per_run);

    info!("{} album(s) to process this cycle", wanted.len());

    for album in &wanted {
        let span = tracing::info_span!(
            "album",
            id = album.id,
            title = %album.title,
            artist = %album.artist.artist_name
        );
        let _enter = span.enter();

        if let Err(e) = process_album(cfg, db, lidarr, slskd, album).await {
            // don't let one bad album take down the whole cycle
            warn!("error processing album: {e:#}");
        }
    }

    // clear completed transfers from slskd's queue at the end of each cycle
    if let Err(e) = slskd.remove_completed_downloads().await {
        warn!("failed to clear completed transfers: {e:#}");
    }

    Ok(())
}

async fn process_album(
    cfg: &Config,
    db: &Blacklist,
    lidarr: &LidarrClient,
    slskd: &SlskdClient,
    album: &WantedAlbum,
) -> Result<()> {
    let query = format!("{} {}", album.artist.artist_name, album.title);
    info!("searching: {query:?}");

    let responses = slskd.search(&query).await?;

    // TODO: fetch expected track count from lidarr for better scoring
    let mut candidates = rank_candidates(
        responses,
        &album.artist.artist_name,
        &album.title,
        None,
        &cfg.kagurarr.preferred_formats,
        cfg.kagurarr.min_score,
    );

    // filter out combos we already know are bad
    candidates.retain(|c| {
        !db.is_blacklisted(album.id, &c.username, &c.remote_folder)
            .unwrap_or(false)
    });

    if candidates.is_empty() {
        info!("no viable candidates (all below threshold or blacklisted)");
        db.insert(
            album.id,
            None,
            None,
            BlacklistStatus::NoMatch,
            Some("no candidates above threshold"),
        )?;
        return Ok(());
    }

    info!("{} candidate(s) to try", candidates.len());

    for candidate in &candidates {
        info!(
            user = %candidate.username,
            score = candidate.score,
            folder = %candidate.remote_folder,
            "trying candidate"
        );

        match try_candidate(cfg, lidarr, slskd, album, candidate).await {
            Ok(ImportResult::Accepted) => {
                info!("lidarr accepted the import");
                db.insert(album.id, None, None, BlacklistStatus::Done, None)?;
                return Ok(());
            }
            Ok(ImportResult::Rejected(reason)) => {
                warn!("lidarr rejected: {reason}");
                db.insert(
                    album.id,
                    Some(&candidate.username),
                    Some(&candidate.remote_folder),
                    BlacklistStatus::Failed,
                    Some("lidarr_rejected"),
                )?;
                // keep going - try the next one
            }
            Err(e) => {
                warn!("candidate failed: {e:#}");
                db.insert(
                    album.id,
                    Some(&candidate.username),
                    Some(&candidate.remote_folder),
                    BlacklistStatus::Failed,
                    Some("download_error"),
                )?;
                // keep going - try the next one
            }
        }
    }

    info!("all candidates exhausted - nothing worked, giving up for now");
    Ok(())
}

async fn try_candidate(
    cfg: &Config,
    lidarr: &LidarrClient,
    slskd: &SlskdClient,
    album: &WantedAlbum,
    candidate: &Candidate,
) -> Result<ImportResult> {
    slskd.download(&candidate.username, &candidate.files).await?;

    // now we wait. fingers crossed.
    let slskd_path = slskd
        .poll_until_done(
            &candidate.username,
            &candidate.files,
            &cfg.slskd.download_dir,
            cfg.kagurarr.stall_timeout_secs,
        )
        .await?;

    // rename the folder to "Artist - Album (Year)" before import.
    // lidarr uses the folder name as the primary artist lookup - a random user's folder name won't match.
    let staged_path = match stage_download(
        &cfg.slskd.download_dir,
        &slskd_path,
        &album.artist.artist_name,
        &album.title,
        album.year(),
    ) {
        Ok(p) => p,
        Err(e) => {
            warn!("couldn't stage download folder: {e:#} - proceeding with original path");
            slskd_path.clone()
        }
    };

    stamp_tags(&staged_path, &album.artist.artist_name, &album.title);

    // lidarr might mount the downloads at a different path than slskd does.
    // if lidarr.download_dir is set, swap the prefix; otherwise use what slskd gave us.
    let lidarr_path = if let Some(lidarr_dir) = &cfg.lidarr.download_dir {
        staged_path.replacen(&cfg.slskd.download_dir, lidarr_dir, 1)
    } else {
        staged_path.clone()
    };

    info!("download done, triggering lidarr import at {lidarr_path}");

    let command_id = lidarr.trigger_import(&lidarr_path).await?;
    let result = lidarr.poll_command(command_id).await?;

    Ok(result)
}

/// move the download folder to a standardized "Artist - Album (Year)" name before import.
/// lidarr's ProcessFolder uses the folder name to look up the artist - a random soulseek folder name won't match.
fn stage_download(
    download_dir: &str,
    current_path: &str,
    artist: &str,
    album: &str,
    year: Option<u32>,
) -> anyhow::Result<String> {
    let folder_name = match year {
        Some(y) => format!("{artist} - {album} ({y})"),
        None => format!("{artist} - {album}"),
    };
    let staging_path = format!(
        "{}/{}",
        download_dir.trim_end_matches('/'),
        sanitize_folder_name(&folder_name)
    );

    info!(
        "staging: current={:?} target={:?} (bytes: {} vs {})",
        current_path,
        staging_path,
        current_path.as_bytes().iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(""),
        staging_path.as_bytes().iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(""),
    );

    if current_path == staging_path {
        info!("paths already match, skipping rename");
        return Ok(staging_path);
    }

    // clean up leftover staging folder from a previous failed attempt
    if std::path::Path::new(&staging_path).exists() {
        std::fs::remove_dir_all(&staging_path)?;
    }

    std::fs::rename(current_path, &staging_path)?;
    info!("staged download folder: {current_path} -> {staging_path}");

    Ok(staging_path)
}

fn sanitize_folder_name(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c => c,
        })
        .collect()
}

/// stamp albumartist + album onto every audio file in the download folder before handing to lidarr.
/// lidarr's DownloadedAlbumsScan is tag-dependent - without correct tags it matches 0 tracks.
/// errors are non-fatal: if tagging fails we warn and let lidarr try anyway.
fn stamp_tags(local_dir: &str, artist: &str, album: &str) {
    let dir = match std::fs::read_dir(local_dir) {
        Ok(d) => d,
        Err(e) => {
            warn!("couldn't read download dir for tagging: {e}");
            return;
        }
    };

    for entry in dir.flatten() {
        let path = entry.path();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        const AUDIO_EXTS: &[&str] = &[
            "flac", "mp3", "ogg", "opus", "m4a", "aac", "wav", "wv", "ape", "aiff",
        ];
        if !AUDIO_EXTS.contains(&ext.as_str()) {
            continue;
        }

        let result = (|| -> anyhow::Result<()> {
            let mut tagged_file = Probe::open(&path)?.read()?;
            let tag = match tagged_file.primary_tag_mut() {
                Some(t) => t,
                None => match tagged_file.first_tag_mut() {
                    Some(t) => t,
                    None => {
                        let tag_type = tagged_file.primary_tag_type();
                        tagged_file.insert_tag(Tag::new(tag_type));
                        tagged_file.primary_tag_mut().unwrap()
                    }
                },
            };

            tag.set_artist(artist.to_string());
            tag.set_album(album.to_string());
            tag.insert(TagItem::new(
                ItemKey::AlbumArtist,
                ItemValue::Text(artist.to_string()),
            ));

            tag.save_to_path(&path, WriteOptions::default())?;
            Ok(())
        })();

        match result {
            Ok(()) => tracing::debug!("tagged: {}", path.display()),
            Err(e) => warn!("couldn't tag {}: {e}", path.display()),
        }
    }
}
