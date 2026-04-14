# kagurarr

[![Primary Repo](https://img.shields.io/badge/primary-Kurobox-purple?logo=forgejo)](https://codex.kurobox.me/miyo-rin/kagurarr)
[![GitHub Mirror](https://img.shields.io/badge/mirror-GitHub-gray?logo=github)](https://github.com/miyo-hime/kagurarr)
![Version](https://img.shields.io/badge/v0.1.6-orange)
![License](https://img.shields.io/badge/license-Apache%202.0-blue)
![Built with Rust](https://img.shields.io/badge/Rust-CE412B?logo=rust&logoColor=white)

a bridge between [Lidarr](https://lidarr.audio) and [slskd](https://github.com/slskd/slskd) (Soulseek daemon) that actually remembers its mistakes.

named after 神楽 (kagura) - the sacred music ritual that bridges human and divine realms. yes it's a bit much for a download manager. no i don't care.

## what is this

you have Lidarr. you have slskd. you want them to talk to each other automatically. that's what this does.

every N minutes it checks Lidarr's wanted list, searches Soulseek for each album, picks the best match it can find, downloads it, stages it, and tells Lidarr to import it. if Lidarr says no - it blacklists that candidate, tries the next one, and never touches the same bad match again.

- **searches `{artist} {album}`** not just the album title - way fewer false positives
- **sqlite blacklist** by `(album_id, user, folder)` - survives restarts
- **configurable minimum score** (default 0.75) - low-confidence matches get skipped
- **blacklist TTL** - failed matches expire after 30 days, new sharers appear
- **stall detection** - if a download stops moving, cancel and try the next candidate
- **automatic cleanup** - leftover failed download folders get deleted after 2h

## you need

- [Lidarr](https://lidarr.audio) set up and running with your music library
- [slskd](https://github.com/slskd/slskd) running with a Soulseek account
- the three containers able to see the same download directory (see below)
- API keys for both

if you don't know what Lidarr or Soulseek are, you've got some reading to do first. this tool assumes you're already past that part.

## setup

```bash
cp config.example.toml data/config.toml
# fill in urls + api keys, adjust the volume path in docker-compose.yml
docker compose up -d
```

see `config.example.toml` for all options. only `[lidarr]` and `[slskd]` are required.

set `RUST_LOG=kagurarr=debug` to see what's happening under the hood.

## the download directory problem

all three containers (kagurarr, slskd, Lidarr) need to be able to see the same physical folder. kagurarr downloads via slskd, stages files there, then tells Lidarr to import from that path.

in `docker-compose.yml`, set the volume to point at slskd's download directory:
```yaml
- /path/to/slskd/downloads:/downloads
```

if Lidarr mounts that directory at a **different path** than `/downloads`, set `download_dir` under `[lidarr]` in your config so kagurarr tells Lidarr the right path. if all three containers use the same mount, you can skip it.

kagurarr doesn't need to run as root. running as the same user that owns your media files is a good idea.

## stack

```
Lidarr  <->  kagurarr  <->  slskd  <->  Soulseek
```

single static binary in a minimal Docker container. written in Rust because i wanted to.
