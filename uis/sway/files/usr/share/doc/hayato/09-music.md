# Hayato Cassette — Music Player

**Hayato Cassette** (`hayato-cassette`, also linked as `hayato-music`) is Hayato OS's
terminal-native music player combining a **streamlined deck + library layout** (Top Source Filter
Tabs `[ All ] [ Spotify ] [ Local ]` + Search Box, Left Library Sidebar, Persistent
`HAYATO // CASSETTE DECK` with Album Art, Transport Controls, Live Visualizer, Scrubber &
Volume, Center Track List, Toggleable Right Drawer, and Shortcut Keybar) with Hayato's
**Cyber-HUD** aesthetic (Sixel album art, 32-band FFT spectrum, and beat-reactive theme borders).

Built on a modular `MusicProvider` architecture (`hayato-music-core/src/providers/`), it
supports **Spotify** (OAuth 2.0 PKCE + 2026 Web API `/items` + Connect live sync + Liked Songs
+ `403` `context_uri` editorial playlist playback) and **Local Audio** (MP3, FLAC, OGG Vorbis,
Opus, WAV, M4A/AAC) side by side.

## Opening & Scratchpad

- Press **`Alt+M`** (or choose **Cassette** in `Alt+Space`) to summon Hayato Cassette.
- Press **`Alt+M`** or **`Esc`** on the library view to hide it to Sway's scratchpad while music keeps playing.
- Press **`q`** to stop playback and quit cleanly (zero background processes).
- Press **`w`** to toggle between the full Cyberdeck (`1120×680`) and the compact **Mini HUD** (`560×300`). Your chosen size persists across Waybar clicks, `Alt+M` toggles, and restarts.

## Controls (Simple Lowercase Keymap)

| Key | Action |
|---|---|
| `Space` | Play / Pause (active provider: Spotify Connect or Local) |
| `n` / `p` | Next / Previous track |
| `[` / `]` | Seek ±5s (`{` / `}` for ±30s) — works from any pane |
| `+` / `-` / `m` | Volume ±5% / Mute |
| `s` / `r` | Shuffle / Repeat (`Off → All → One`) |
| `F5` / `Ctrl+r` | Refresh playlists/devices & rescan local `~/Music` |
| `/` or `Ctrl+k` | Focus Search card |
| `d` | Open Audio Output / Device Picker |
| `l` | Toggle Synced Lyrics (local `.lrc` + LRCLIB online cache) |
| `o` or `,` | Toggle Options / Settings |
| `v` / `f` | Cycle Visualizer mode / Fullscreen Visualizer |
| `b` / `u` | Toggle Library Sidebar / Up Next Queue |
| `Tab` / `←` / `→` | Switch focus between visible panes |
| `Enter` / `a` | Play track (or open playlist) / Add track to queue |
| `Ctrl+s` | Share current track or Spotify link via **Hayato Share** |
| `:` / `?` / `q` | Command Palette / Keymap Help / Quit |

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
2. Press **`Enter`** on **Sign in with Spotify (Browser PKCE)** — your browser opens to authorize
   and returns automatically.
3. To play Spotify audio directly on **this Chromebook** (instead of remote speakers like Echo
   or your phone), press **`Enter`** on **This Device Audio (Spotify Connect)** (or press
   **`D`** → **`E`** in the Device Picker) for 1-click pairing — or tap **`Hayato Cassette`**
   in Spotify on your phone on the same Wi-Fi. Once paired, Cassette defaults Spotify playback
   to **This Device** (`● This Device ▾`) on every launch and shuts down cleanly when closed.
4. Credentials are stored strictly at `~/.config/hayato/cassette/` (`0600`).
