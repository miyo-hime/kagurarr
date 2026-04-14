# Kagurarr

A Rust replacement for Soularr — named after 神楽 (kagura), the sacred music ritual that bridges the human and divine realms. — a bridge between Lidarr and slskd (Soulseek daemon).

## What this replaces

[Soularr](https://github.com/mrusse/soularr) is a Python script that bridges Lidarr's wanted list with slskd to download music from Soulseek. It works, but has enough rough edges that building something better in Rust is worthwhile.

---

## How the current stack works

```
Lidarr (benten.kurobox.me)
  ↕ wanted list + import triggers
kagurarr  ← this project
  ↕ search + download triggers
slskd (raijin.kurobox.me)
  ↕ Soulseek P2P network
~/docker/slskd/downloads/
  ↕ Lidarr imports files, moves to:
~/music/
  ↕ read-only mount
Navidrome (fujin.kurobox.me)
```

### API surface we've confirmed working

**Lidarr** (`http://lidarr:8686`, header: `X-API-Key`):
- `GET  /api/v1/wanted/missing` — paginated list of wanted albums
- `GET  /api/v1/album?artistId={id}` — all albums for an artist
- `GET  /api/v1/artist` — all artists
- `POST /api/v1/command` `{"name":"DownloadedAlbumsScan","path":"/downloads/..."}` — trigger import
- `POST /api/v1/command` `{"name":"RescanArtist","authorId":N}` — rescan artist files
- `GET  /api/v1/rename?artistId={id}` — preview rename plan (returns existingPath + newPath pairs)
- `GET  /api/v1/trackFile?artistId={id}` — files Lidarr knows about
- `GET  /api/v1/queue` — current download queue

**slskd** (`http://slskd:5030`, header: `X-API-Key`):
- `POST /api/v0/searches` `{"searchText":"...","id":"<uuid>"}` — start a search
- `GET  /api/v0/searches/{id}` — poll search status (check `isComplete`)
- `GET  /api/v0/searches/{id}/responses` — get results grouped by user
- `POST /api/v0/transfers/downloads` — queue files for download
- `GET  /api/v0/transfers/downloads` — check download status

**slskd search response shape:**
```json
{
  "username": "soliva",
  "files": [
    {
      "filename": "\\path\\to\\file.flac",
      "size": 12345678,
      "extension": "flac"
    }
  ]
}
```

---

## Problems with Soularr (what motivated this)

### 1. No blacklisting of failed matches
This is the big one. When Lidarr rejects a download (wrong content, bad metadata match), Soularr just moves it to `failed_imports/` and tries again next cycle. For albums with generic names, this means it grabs the same wrong content on repeat forever.

**Fix:** persist a blacklist of `(lidarr_album_id, slskd_username, remote_folder)` tuples that failed. On next search, skip those candidates and try the next best match.

### 2. Generic album names cause false positive matches
Album titles like `kirin`, `bedtime story`, `the Radio`, `the Post` return hundreds of results on Soulseek with no relation to the target artist. Soularr's matching is based on track count + filename sequence similarity, which breaks down when the result set is noisy.

**Real examples of wrong grabs:**
- `kirin` → grabbed `Excavated Anthems` (Jaska087, a Touhou doujin album with matching track count)
- `bedtime story` → grabbed `Robert Glasper Experiment - ArtScience`
- `the Radio` → grabbed Ace Of Base tracks

**Fix:** search for `{artist} {album}` not just `{album}`. Also apply a minimum match ratio threshold — Soularr found these at 0.61-0.65; anything below ~0.75 should be skipped or flagged.

### 3. Retries wrong matches infinitely
Related to #1 — even if you clean up `failed_imports/`, Soularr will grab the same wrong folder again. No memory between runs.

### 4. No configurable match quality threshold
The sequence match ratio is computed but never used as a gate. Matches at 0.61 get treated the same as matches at 0.88.

### 5. Tries to write music tags to folder.jpg
Soularr iterates all files in a download folder and tries to tag them, including cover art images. Results in a noisy (but non-fatal) error every run:
```
NotImplementedError: Mutagen type <class 'NoneType'> not implemented
```

### 6. Config section names are case-sensitive and undocumented
`[lidarr]` crashes — must be `[Lidarr]`, `[Slskd]`, `[Soularr]`. The README doesn't mention this. Took a traceback to find it.

### 7. Docker image not on Docker Hub
`mrusse/soularr` doesn't exist on Docker Hub — it's at `ghcr.io/mrusse/soularr`. The docs don't make this clear.

### 8. Stale lock file problem
Soularr uses a lock file to prevent concurrent runs. If a process is killed mid-run (e.g., manual `docker exec` that gets interrupted), the lock isn't cleaned up and the daemon silently skips all future runs with "Soularr is already running."

### 9. Lidarr `RenameArtist` command silently does nothing (unrelated, but discovered during setup)
Lidarr v3.1.0.4875 — the rename command completes instantly with no log output and no files moved. Had to script the renames manually using the `/api/v1/rename` preview endpoint + Python `shutil.move`.

---

## Architecture for kagurarr

### Overview

```
┌─────────────────────────────────────────────┐
│                kagurarr                   │
│                                             │
│  Scheduler loop (configurable interval)     │
│    │                                        │
│    ├─ LidarrClient::wanted_albums()         │
│    │    filter: not in blacklist as "done"  │
│    │                                        │
│    ├─ for each wanted album:                │
│    │    SlskdClient::search(artist, album)  │
│    │    score_candidates()                  │
│    │    filter blacklisted (album, user,    │
│    │      folder) combos                    │
│    │    pick best remaining candidate       │
│    │    SlskdClient::download(files)        │
│    │    poll until complete                 │
│    │    LidarrClient::import(path)          │
│    │    → success: blacklist as "done"      │
│    │    → failure: blacklist this candidate │
│    │               try next candidate       │
│    │                                        │
│    └─ sleep(interval)                       │
│                                             │
│  Blacklist: SQLite (one table)              │
└─────────────────────────────────────────────┘
```

### SQLite schema

```sql
CREATE TABLE blacklist (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    lidarr_album_id INTEGER NOT NULL,
    slskd_username  TEXT,           -- NULL means "no match found at all"
    remote_folder   TEXT,           -- NULL means "no match found at all"
    status          TEXT NOT NULL,  -- 'failed', 'done', 'no_match'
    reason          TEXT,           -- human-readable, e.g. "lidarr_rejected", "wrong_content"
    attempted_at    TEXT NOT NULL   -- ISO8601 timestamp
);

CREATE INDEX idx_blacklist_album ON blacklist(lidarr_album_id, status);
```

### Candidate scoring

For each search result, score the candidate:

```rust
struct Candidate {
    username: String,
    remote_folder: String,
    files: Vec<SlskdFile>,
    score: f64,
}

fn score_candidate(candidate: &Candidate, wanted: &WantedAlbum) -> f64 {
    // 1. track count match (hard filter or heavy penalty)
    // 2. file format (flac > mp3 > other)
    // 3. sequence similarity of filenames to expected track titles
    // 4. folder name similarity to "{artist} - {album}"
    // 5. bonus: username in a known-good list (optional)
}
```

Reject any candidate below a configurable `min_score` (suggest 0.75 default).

### Config (TOML)

```toml
[lidarr]
url = "http://lidarr:8686"
api_key = "..."

[slskd]
url = "http://slskd:5030"
api_key = "..."
download_dir = "/downloads"

[soularr]
interval_secs = 600
min_score = 0.75
preferred_formats = ["flac", "mp3"]
max_albums_per_run = 10

[database]
path = "/data/soularr.db"
```

### Suggested crate choices

| Need | Crate |
|------|-------|
| HTTP client | `reqwest` (async) |
| Async runtime | `tokio` |
| SQLite | `rusqlite` (well-maintained, good bundled feature) |
| TOML config | `toml` + `serde` |
| Logging | `tracing` + `tracing-subscriber` |
| Error handling | `anyhow` |
| String similarity | `strsim` (sequence_matcher equivalent) |
| UUID for search IDs | `uuid` |

### Improvements over Soularr (summary)

- **Blacklist by (album_id, user, folder)** — never retry a known-bad match
- **Search by artist + album** — dramatically fewer false positives
- **Minimum score threshold** — configurable, default 0.75
- **Exhaust candidates before giving up** — try next-best match on failure, don't just quit
- **Skip non-audio files when tagging** — no folder.jpg errors
- **TOML config** — case-insensitive keys, serde-validated, sane defaults
- **No lock file** — tokio handles concurrency, single async task per run
- **Proper error types** — distinguish "Lidarr rejected" vs "download stalled" vs "no candidates"
- **Structured logging** — tracing spans per album so logs are readable
- **Clean Docker image** — single static binary, minimal base image

---

## kurobox deployment notes

- Container runs on `proxy` Docker network (can reach `lidarr` and `slskd` by container name)
- Runs as `user: "1000:1000"` (miyo) — critical, otherwise downloaded files are root-owned and Lidarr can't import
- Downloads land in `/downloads` → `~/docker/slskd/downloads/` (shared volume with slskd and lidarr)
- Music root: `/music` → `~/music/` (write access for Lidarr to import)
- Config at `/data/config.toml`, DB at `/data/soularr.db`
- kami aliases: `soularr` (replace current container)

### Volumes (same as current Soularr)

```yaml
volumes:
  - ./data:/data
  - /home/miyo/docker/slskd/downloads:/downloads
```

---

## Things to decide / open questions

- Should the blacklist have a TTL? (e.g. retry failed matches after 30 days in case new sharers appear)
- Should we keep a "known good users" list for J-music? (soliva came up twice with correct matches)
- Do we want a simple status endpoint / healthcheck? (probably yes for homepage widget)
- Build and deploy via Forgejo Actions on codex.kurobox.me?
