# wbunpacker (wbu)

[中文版](./docs/README.zh-CN.md)

Asset extraction and unpacking tool for **Shadowverse: Worlds Beyond** — downloads, decrypts, parses, and exports game assets from the CDN.

## Features

- **Manifest** — Download and parse resource manifests; diff two versions to track asset changes
- **Asset** — Concurrent download, XOR decryption, and blob-storage deduplication of Unity AssetBundles
- **Master Data** — Export 173 MasterMemory tables to JSON; generate derived data (cards, packs, emblems, etc.)
- **Audio** — Extract Wwise WEM from AKPK containers, decode to WAV (vgmstream) and transcode to MP3 (ffmpeg); extract card and leader-skin voices
- **Texture** — Extract card art (848×1024), card backs / sleeves (764×1024), pack icons, card frames, crests, emblems, stamps, and home-illustration pictures; render full card images from layers
- **Spine Animations** — Extract HomeIllustration and LeaderSkin Spine skeletons, atlases, and textures
- **MetaDB** — Decrypt the client `meta.db` (SQLite3MC) via dynamic DLL loading

## Requirements

- **Rust** nightly (edition 2024)
- [AssetStudioModCLI](https://github.com/aelurum/AssetStudio) — Unity AssetBundle extraction
- [vgmstream-cli](https://github.com/vgmstream/vgmstream) — WEM to WAV decoding
- [ffmpeg](https://ffmpeg.org/) — WAV to MP3 transcoding (optional)
- `libnative.dll` from the game installation — MetaDB decryption (optional)

## Setup

```bash
# Clone
git clone <repo-url>
cd wbunpacker

# Copy and edit config
cp config/config.example.toml config/Config.local.toml
```

Fill in `config/Config.local.toml`:

| Key | Description |
|-----|-------------|
| `data_dir` | Output root directory |
| `default_version` | Manifest version hash (from CDN URL) |
| `asset_bundle_base_keys` | Decryption base keys (base64) |
| `asset_studio_path` | Path to AssetStudioModCLI executable |
| `vgmstream_path` | Path to vgmstream-cli |
| `ffmpeg_path` | Path to ffmpeg |
| `manifest_address` | Manifest CDN URL template |
| `asset_bundle_address` | AssetBundle CDN URL template |

You can also set `WBU_CONFIG` env var to point to a custom config path.

## Install

### Pre-built binaries

Download the latest `wbunpacker-<version>-<target>.tar.gz` (or `.zip` for Windows) from the [Releases](https://github.com/Hypnos-PD/wbunpacker/releases) page, then extract:

```bash
# Linux / macOS
tar xzf wbunpacker-v0.1.0-x86_64-unknown-linux-gnu.tar.gz
cd wbunpacker-v0.1.0-x86_64-unknown-linux-gnu

# Windows (PowerShell)
Expand-Archive wbunpacker-v0.1.0-x86_64-pc-windows-msvc.zip
cd wbunpacker-v0.1.0-x86_64-pc-windows-msvc
```

The archive contains:

```
wbu                     # (or wbu.exe on Windows)
config/
  config.example.toml   # Configuration template
```

Copy and edit the config file, then run:

```bash
cp config/config.example.toml config/Config.local.toml
# edit config/Config.local.toml with your settings
./wbu --help
```

The binary looks for `config/Config.local.toml` in the current working directory, so run it from the extracted folder (or set the `WBU_CONFIG` environment variable to a custom path).

> **Note:** The external tools listed in [Requirements](#requirements) (AssetStudioModCLI, vgmstream-cli, ffmpeg, libnative.dll) are still needed — they are not bundled.

### Build from source

```bash
git clone <repo-url>
cd wbunpacker
cp config/config.example.toml config/Config.local.toml
# edit config/Config.local.toml
cargo build --release
# binary: target/release/wbu
```

## Usage

### Manifest

```bash
wbu manifest -v Chs                      # Download & parse manifest (Chinese)
wbu manifest -v Eng --format json        # Export as JSON
wbu manifest diff -o old_rev -n new_rev  # Diff two versions
wbu manifest diff -n latest -t 30        # Show top 30 changed items vs. repo
```

### Assets

```bash
wbu asset download <name> -v Chs         # Download single asset by name
wbu asset decrypt <file>                 # Decrypt a single .ab file
wbu asset batch -v Chs                   # Download all assets for a variant
wbu asset batch -v Chs -c 16             # With 16 concurrent downloads
wbu asset batch -v Chs --diff            # Download only changed assets from diff
wbu asset batch -v Chs --diff --extract  # Also auto-extract with AssetStudioModCLI
```

### Master Data

```bash
wbu master -v all                    # Export all 173 tables to JSON
wbu master cards                     # Generate cards_full.json (merged card data)
wbu master packs                     # Generate pack_names.json
wbu master emblems                   # Generate emblems_full.json
wbu master stamps                    # Generate stamps_full.json
wbu master sleeves                   # Generate sleeves_full.json (card backs)
```

### Audio

```bash
wbu audio                            # Build Wwise mapping + extract AKPK → WEM → WAV
wbu audio --mp3                      # Also transcode WAV to MP3
wbu audio card                       # Extract card voices (MP3 + voice_index.json)
wbu audio card -F                    # Force overwrite existing files
wbu audio leader-skin                # Extract LeaderSkin detail voices
```

### Textures & Rendering

```bash
wbu texture card                     # Export card art textures (848×1024)
wbu texture pack-icons               # Extract pack icons
wbu texture card-frames              # Extract Card2D frames (PNG)
wbu texture crests                   # Extract crest/faith icons
wbu texture emblems                  # Extract emblem textures
wbu texture stamps                   # Extract stamp textures
wbu texture sleeves                  # Extract card back / sleeve textures (764×1024)
wbu texture home-illust-picts        # Extract home illustration static images
wbu render cards                     # Batch render full card images
wbu render card --id 100101          # Render a single card
```

### Spine Animations

```bash
wbu home-illust                      # Extract all HomeIllustration Spine animations
wbu home-illust --voices             # Copy voice files alongside animations
wbu leader-skin -v Chs               # Extract LeaderSkin Spine animations (Chinese names)
```

### MetaDB

```bash
wbu metadb meta.db -o meta_decrypted.db --dll ./libnative.dll
```

## Typical Workflow

```bash
wbu manifest -v Chs --format json     # 1. Download & parse manifest
wbu asset batch -v Chs                # 2. Download all AssetBundles
wbu master -v all                     # 3. Export master data tables
wbu master cards                      # 4. Generate card data
wbu audio                             # 5. Extract audio
wbu texture card                      # 6. Extract card textures
wbu texture card-frames               # 7. Extract card frames
wbu render cards                      # 8. Render full card images
```

## Output Structure

```
<data_dir>/
├── manifests/                          # Downloaded manifest .raw files
├── manifest-json/                      # Parsed manifest JSONs
├── manifest-diffs/                     # Diff output JSONs
├── blobs/                              # Raw AssetBundles (hash-keyed, deduplicated)
├── variants/<variant>/                 # Hardlinks to blobs per language variant
├── audio/
│   ├── WwiseIdMapping/                 # Decrypted Wwise event mappings
│   ├── akpk/                           # Extracted AKPK containers
│   ├── wem/                            # Extracted WEM audio chunks
│   └── wav/                            # Decoded WAV files
├── master/                             # MasterMemory tables (JSON)
├── derived/                            # Generated data (cards_full.json, etc.)
├── textures/                           # Extracted card art, icons, frames
├── sleeves/                            # Extracted card backs / sleeves
│   ├── raw/                             #   Original (1024×1024)
│   └── resized/                         #   Resized (764×1024)
├── homeillust/                         # HomeIllustration Spine animations
├── leaderskin/                         # LeaderSkin Spine animations
└── rendered/                           # Final rendered card images
```

## License

MIT
