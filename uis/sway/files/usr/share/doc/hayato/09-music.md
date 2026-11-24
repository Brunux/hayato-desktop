# Hayato Cassette — Music Player

**Hayato Cassette** (`hayato-cassette`, also linked as `hayato-music`) is Hayato OS's
terminal-native cyberdeck music player for both your **local music library** (MP3, FLAC,
OGG Vorbis, Opus, WAV, M4A/AAC) and **Spotify**.

## Opening & Scratchpad

- Press **`Alt+M`** (or choose **Cassette** in `Alt+Space`) to summon Hayato Cassette.
- Press **`Alt+M`** or **`Esc`** on the main Deck to hide it to Sway's scratchpad while
  music keeps playing in the background.
- Press **`Q`** to stop playback and quit cleanly (zero background processes left behind).
- Press **`m`** to toggle between the full Cyberdeck (`120×36`) and the compact **Mini HUD** (`60×16`).

## Controls

| Key | Action |
|---|---|
| `Space` | Play / Pause |
| `n` / `p` | Next / Previous track |
| `←` / `→` | Seek ±5s (`Shift+←/→` for ±30s) |
| `+` / `-` / `M` | Volume ±5% / Mute |
| `1`–`6`, `,` | Switch tabs: Deck, Library, Queue, Search, Playlists, Spotify, Settings |
| `/` | Unified Local + Spotify Search |
| `:` or `Ctrl+P` | Command Palette |
| `v` | Cycle Visualizer (`Spectrum → Oscilloscope → Off`) |
| `L` | Toggle synced `.lrc` lyrics drawer |
| `z` / `r` | Shuffle / Repeat (`Off → All → One`) |
| `Ctrl+S` | Share current track or Spotify link via **Hayato Share** |
| `J` / `K`, `d`, `c`, `s` | Queue: reorder, remove, clear (confirms first), save `.m3u8` |

## Waybar & Media Keys

While Cassette is playing or paused, a compact **`♫ <track>`** pill appears on Waybar
with a micro-HUD tooltip (`CASSETTE // PLAYING`):
- **Left-click** toggles the Cassette window.
- **Right-click** plays/pauses.
- **Scroll up/down** skips next/previous.

Cassette exposes a standard MPRIS2 service (`org.mpris.MediaPlayer2.hayato_cassette`), so
hardware media keys (`XF86AudioPlay`, `Next`, `Prev`) and `playerctl` control it natively.

## Spotify Setup

1. Open tab **`6 Spotify`** and press **`c`** to paste your Spotify Developer Dashboard Client ID
   (set redirect URI to `http://127.0.0.1:8898/callback`).
2. Press **`Enter`** on **Sign in with Spotify (Browser OAuth PKCE)** — Firefox opens to authorize
   and returns automatically.
3. Tokens are stored strictly at `~/.config/hayato/cassette/spotify.json` (`0600`).
