use crate::slskd::{SearchResponse, SlskdFile};
use strsim::jaro_winkler;

/// a candidate download - one user's folder worth of files
#[derive(Debug, Clone)]
pub struct Candidate {
    pub username: String,
    pub remote_folder: String,
    pub files: Vec<SlskdFile>,
    pub score: f64,
}

/// score and filter candidates from a slskd search response
pub fn rank_candidates(
    responses: Vec<SearchResponse>,
    artist: &str,
    album: &str,
    expected_tracks: Option<usize>,
    preferred_formats: &[String],
    min_score: f64,
) -> Vec<Candidate> {
    let mut candidates: Vec<Candidate> = responses
        .into_iter()
        .filter_map(|resp| {
            // group files by folder (backslash-separated windows paths from soulseek)
            let folder = extract_folder(&resp.files.first()?.filename);
            let score = score_candidate(&resp.files, &folder, artist, album, expected_tracks, preferred_formats);

            Some(Candidate {
                username: resp.username,
                remote_folder: folder,
                files: resp.files,
                score,
            })
        })
        .filter(|c| c.score >= min_score)
        .collect();

    // best first
    candidates.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    candidates
}

fn score_candidate(
    files: &[SlskdFile],
    folder: &str,
    artist: &str,
    album: &str,
    expected_tracks: Option<usize>,
    preferred_formats: &[String],
) -> f64 {
    let mut score = 0.0_f64;

    // only count audio files - skip folder.jpg and friends
    let audio_files: Vec<&SlskdFile> = files.iter().filter(|f| is_audio(f)).collect();

    if audio_files.is_empty() {
        return 0.0;
    }

    // track count match (0.0-0.3)
    if let Some(expected) = expected_tracks {
        let ratio = audio_files.len() as f64 / expected as f64;
        // penalize if we're way off, but give some slack for bonus tracks etc.
        let track_score = if ratio >= 0.8 && ratio <= 1.3 {
            0.3
        } else if ratio >= 0.6 {
            0.15
        } else {
            0.0 // hard pass - track count is too far off
        };
        score += track_score;
    } else {
        // no expected track count - give partial credit for having some tracks
        score += 0.15;
    }

    // format quality (0.0-0.2)
    let dominant_format = dominant_format(audio_files.as_slice());
    if let Some(fmt) = &dominant_format {
        if let Some(pos) = preferred_formats.iter().position(|f| f == fmt) {
            // first preferred format gets full points, degrades after that
            score += 0.2 / (pos as f64 + 1.0);
        }
    }

    // folder name similarity to "{artist} - {album}" (0.0-0.3)
    let folder_lower = folder.to_lowercase();
    let target = format!("{} {}", artist, album).to_lowercase();
    let folder_sim = jaro_winkler(&folder_lower, &target);
    score += folder_sim * 0.3;

    // artist name present in folder (0.0-0.2)
    if folder_lower.contains(&artist.to_lowercase()) {
        score += 0.2;
    }

    score.min(1.0)
}

fn extract_folder(filename: &str) -> String {
    // soulseek paths are backslash-separated windows paths
    // e.g. "\\server\share\Artist - Album\01 - Track.flac"
    let parts: Vec<&str> = filename.split('\\').collect();
    if parts.len() > 1 {
        parts[..parts.len() - 1].join("\\")
    } else {
        // no folder separator - just use the bare filename
        filename.to_string()
    }
}

fn is_audio(file: &SlskdFile) -> bool {
    let audio_exts = ["flac", "mp3", "aac", "ogg", "opus", "wav", "m4a", "wv", "ape"];
    match &file.extension {
        Some(ext) => audio_exts.contains(&ext.to_lowercase().as_str()),
        None => {
            // fall back to checking the filename itself
            let lower = file.filename.to_lowercase();
            audio_exts.iter().any(|ext| lower.ends_with(ext))
        }
    }
}

fn dominant_format(files: &[&SlskdFile]) -> Option<String> {
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for f in files {
        if let Some(ext) = &f.extension {
            *counts.entry(ext.to_lowercase()).or_insert(0) += 1;
        }
    }
    counts.into_iter().max_by_key(|(_, v)| *v).map(|(k, _)| k)
}
