# Changelog

## [0.1.0] - 2026-08-08

### Added

- **Manifest** — Download & parse CDN resource manifests; diff two versions to track asset changes (added/removed/modified)
- **Asset** — Concurrent download of Unity AssetBundles with XOR decryption and blob-storage deduplication; batch download with diff mode and auto-extraction via AssetStudioModCLI
- **Master Data** — Export 173 MasterMemory tables to JSON; generate derived data: cards, packs, emblems, crests, stamps
- **Audio** — Extract Wwise WEM from AKPK containers; decode to WAV (vgmstream) and transcode to MP3 (ffmpeg); extract card voices and leader-skin detail voices
- **Texture** — Extract card art (848×1024), pack icons, card frames, crests, emblems, stamps, and home-illustration pictures
- **Spine Animations** — Extract HomeIllustration and LeaderSkin Spine skeletons, atlases, and textures
- **MetaDB** — Decrypt client `meta.db` (SQLite3MC) via dynamic DLL loading with `libnative.dll`
- **Card Rendering** — Batch render full card images composited from layers
- **CI/CD** — GitHub Actions for build, test, clippy, and multi-platform release (Linux x86_64/aarch64, Windows x86_64, macOS x86_64/aarch64)
