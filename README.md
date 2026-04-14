# kagurarr

[![Primary Repo](https://img.shields.io/badge/primary-Kurobox-purple?logo=forgejo)](https://codex.kurobox.me/miyo-rin/kagurarr)
[![GitHub Mirror](https://img.shields.io/badge/mirror-GitHub-gray?logo=github)](https://github.com/miyo-hime/kagurarr)
![Version](https://img.shields.io/badge/v0.1.0-orange)
![License](https://img.shields.io/badge/license-Apache%202.0-blue)
![Built with Rust](https://img.shields.io/badge/Rust-CE412B?logo=rust&logoColor=white)

a bridge between [Lidarr](https://lidarr.audio) and [slskd](https://github.com/slskd/slskd) (Soulseek daemon) that actually remembers its mistakes.

named after 神楽 (kagura) - the sacred music ritual that bridges human and divine realms. yes it's a bit much for a download manager. no i don't care.

## what is this

you have Lidarr. you have slskd. you want them to talk to each other automatically. that's what this does.

every N minutes it checks Lidarr's wanted list, searches Soulseek for each album, picks the best match it can find, downloads it, and tells Lidarr to import it. if Lidarr goes "nope, wrong album" - it remembers that, blacklists the candidate, and tries the next best option instead of just... grabbing the same wrong thing again forever.

- **searches `{artist} {album}`** not just the album title - way fewer false positives
- **sqlite blacklist** by `(album_id, user, folder)` - survives restarts, never retries a known-bad match
- **configurable minimum score** (default 0.75) - low-confidence matches get skipped
- **works through candidates** before giving up - rejection just means "try the next one"
- **blacklist TTL** - failed matches expire after 30 days, because new sharers appear
- **stall detection** - if a download stops making progress, cancel it and move on
- **structured logging** via `tracing` so you can actually tell what it's doing

## setup

```bash
cp config.example.toml data/config.toml
# fill in your api keys
docker compose up -d
```

see `config.example.toml` for all the options. only `[lidarr]` and `[slskd]` are required, everything else has sane defaults.

set `RUST_LOG=kagurarr=debug` if you want to see what's going on under the hood.

## docker notes

the `/downloads` volume needs to be the same physical directory that slskd downloads into AND that Lidarr watches for imports - all three containers need to agree on where that is. if Lidarr can't see the files kagurarr just downloaded, imports will silently fail.

if you're on a network where your containers can reach each other by name (e.g. a shared Docker network), you can use container names directly in the urls - `http://lidarr:8686`, `http://slskd:5030`. otherwise use IPs or hostnames.

kagurarr doesn't need to run as root. running it as the same user that owns your media files is a good idea.

## stack

```
Lidarr  <->  kagurarr  <->  slskd  <->  Soulseek
```

single static binary in a minimal Docker container. written in Rust because i wanted to.
