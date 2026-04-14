use anyhow::Result;
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
    _album: &WantedAlbum,
    candidate: &Candidate,
) -> Result<ImportResult> {
    slskd.download(&candidate.username, &candidate.files).await?;

    // now we wait. fingers crossed.
    let local_path = slskd
        .poll_until_done(
            &candidate.username,
            &candidate.files,
            &cfg.slskd.download_dir,
            cfg.kagurarr.stall_timeout_secs,
        )
        .await?;

    info!("download done, triggering lidarr import at {local_path}");

    let command_id = lidarr.trigger_import(&local_path).await?;
    let result = lidarr.poll_command(command_id).await?;

    Ok(result)
}
