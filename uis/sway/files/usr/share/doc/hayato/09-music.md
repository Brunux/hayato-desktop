# Hayato Cassette — Music Player

**Hayato Cassette** (`hayato-cassette`, also linked as `hayato-music`) is Hayato OS's
terminal-native music player combining a **Spotify Web Player-inspired layout** (Left Library
Sidebar, Center Main Stage, Always-Visible Top Search Box, Toggleable Right Drawer, and
Persistent Bottom Player Bar) with Hayato's **Cyber-HUD** aesthetic (Sixel album art,
`tui-big-text`, 32-band FFT spectrum, and beat-reactive theme borders).

Built on a modular `MusicProvider` architecture (`hayato-music-core/src/providers/`), it
supports **Spotify** (OAuth 2.0 PKCE + 2026 Web API `/items` + Connect live sync + Liked Songs
+ `403` `context_uri` editorial playlist playback) and **Local Audio** (MP3, FLAC, OGG Vorbis,
Opus, WAV, M4A/AAC) side by side.

## Opening & Scratchpad

- Press **`Alt+M`** (or choose **Cassette** in `Alt+Space`) to summon Hayato Cassette.
- Press **`Alt+M`** or **`Esc`** on Home to hide it to Sway's scratchpad while music keeps playing.
- Press **`Q`** to stop playback and quit cleanly (zero background processes).
- Press **`w`** to toggle between the full Cyberdeck (`1120×680`) and the compact **Mini HUD** (`560×300`).

## Controls (Zero-Conflict Keymap)

| Key | Action |
|---|---|
| `Space` | Play / Pause (active provider: Spotify Connect or Local) |
| `n` / `p` | Next / Previous track |
| `[` / `]` | Seek ±5s (`{` / `}` for ±30s) — works from any pane |
| `+` / `-` / `m` | Volume ±5% / Mute |
| `z` / `r` | Shuffle / Repeat (`Off → All → One`) |
| `R` / `F5` | Refresh Spotify playlists/devices & rescan local `~/Music` |
| `/` or `Ctrl+K` | Focus always-visible Top Search Box (auto-switches to Search) |
| `D` | Open Spotify Connect Device Picker |
| `1` / `2` / `3` / `4` | Home / Focus Library Sidebar / Queue Drawer / Settings |
| `b` / `i` / `q` / `L` | Toggle Left Sidebar / Now Playing Drawer / Queue / Synced `.lrc` Lyrics |
| `v` / `V` | Cycle Visualizer (`Spectrum → Mirror EQ → Oscilloscope → VU Deck → Cyber Plasma → Off`) / Full-Center Visualizer |
| `Tab` / `h` / `l` | Switch focus between visible panes |
| `Enter` / `P` | Open playlist (or play track) / Play entire playlist immediately |
| `a` / `A` | Enqueue track / Play Next |
| `J` / `K`, `d`, `Ctrl+X`, `S` | Queue: reorder, remove, clear (confirms first), save `.m3u8` |
| `Ctrl+S` | Share current track or Spotify link via **Hayato Share** |
| `:` / `Ctrl+P` / `?` | Command Palette / Keymap Help |

## Waybar & Media Keys

While Cassette is playing or paused, a compact **`♫ <track>`** pill appears on Waybar
with a micro-HUD tooltip (`CASSETTE // PLAYING`):
- **Left-click** toggles the Cassette window.
- **Right-click** plays/pauses.
- **Scroll up/down** skips next/previous.

Cassette exposes a standard MPRIS2 service (`org.mpris.MediaPlayer2.hayato_cassette`), so
hardware media keys (`XF86AudioPlay`, `Next`, `Prev`) and `playerctl` control it natively.

## Spotify Setup & This Device Audio

1. Press **`4`** (Settings) and choose **Spotify Developer Client ID** to paste your Client ID
   (set redirect URI to `http://127.0.0.1:8898/callback`).
2. Press **`Enter`** on **Sign in with Spotify (Browser PKCE)** — Firefox opens to authorize
   and returns automatically.
3. To play Spotify audio directly on **this Chromebook** (instead of remote speakers like Echo
   or your phone), press **`Enter`** on **This Device Audio (Spotify Connect)** (or press
   **`D`** → **`E`** in the Device Picker) for 1-click pairing — or tap **`Hayato Cassette`**
   in Spotify on your phone on the same Wi-Fi. Once paired, Cassette defaults Spotify playback
   to **This Device** (`● This Device ▾`) on every launch and shuts down cleanly when closed.
4. Credentials are stored strictly at `~/.config/hayato/cassette/` (`0600`).
