# The Files window

**Hayato Files** (`hayato-filer`) is the native GTK4 file browser of the session,
bound to **`Alt+Ctrl+Shift+F`**. It is the windowed companion of the terminal
list (`hayato menu files`, the old `hayato-files`): the list is one folder with a
preview beside it, and this is a Finder / elementary OS window with a sidebar,
tabs, a grid or a list, and drag-and-drop.

It reads the filesystem directly (no index, no daemon), draws its icons from the
session's Font Awesome alphabet, and follows the live theme like every other
Hayato surface (`hayato theme set` re-themes it in place over SIGUSR1).

## Layout

* **Header** - the sidebar toggle, back / forward, the breadcrumb path (click
  **Ctrl+L** to type a path instead), the filter box (**Ctrl+F**), the grid /
  list switch (**Ctrl+1** / **Ctrl+2**), the sort menu, the shelf toggle, and
  **New Tab**.
* **Tab bar** - appears as soon as a second tab exists. A tab is dropped on as
  a folder, and hovering one while dragging spring-loads it.
* **PLACES** - Home, Desktop, Documents, Downloads, Pictures, Music, Videos,
  Trash, and your pinned folders. **DEVICES** - removable drives, with a usage
  bar, a mount / eject button, and right-click **Format Drive…**.
* **Canvas** - the icon grid or the compact list; **Backspace** goes up a level.
* **Shelf** - the staging tray along the bottom (see below).
* **Status bar** - the item count, the selected size, and the free space of the
  current filesystem, plus the progress bar for a running copy, move or archive.

## Keys

| Key | Action |
|---|---|
| `Enter` | open the selection (a folder, or the file's program) |
| `Alt+Enter` / `Ctrl+I` | Quick Properties (see below) |
| `Space` | Quick Look preview |
| `Ctrl+Z` | undo the last rename, move, trash, copy or folder creation |
| `F2` | rename |
| `Delete` | move to Trash (`Shift+Delete` deletes for good) |
| `Ctrl+C` / `Ctrl+X` / `Ctrl+V` | copy / cut / paste files |
| `Ctrl+H` | show hidden files |
| `Ctrl+A` | select all |
| `Ctrl+T` / `Ctrl+W` | new tab / close tab (`Ctrl+Tab` cycles, `Alt+1…9` jumps) |
| `Ctrl+F` | filter the folder as you type |
| `Ctrl+L` | type a path |
| `Ctrl+B` / `F9` | show or hide the sidebar |
| `Ctrl+R` / `F5` | reload (folders also refresh themselves) |
| `Ctrl+Shift+N` | new folder |
| `Ctrl+Shift+T` | terminal in this folder |
| `Ctrl+Shift+C` | copy the path(s) |
| `Ctrl+Shift+S` | share the selection through Hayato Share |
| `Ctrl+Q` | close the window |

Middle-click opens a folder in a new tab; right-click anywhere for the context
menu, which carries the Hayato tools (Share, Play in Cassette, Scan / OCR, Search
Inside Folder, Pin to Sidebar, More Actions).

## It keeps up on its own

The open folder is watched with inotify, so a file a browser downloaded, a
terminal wrote, or another app moved appears in place without an **F5**. Bursts
of writes are coalesced into one refresh.

## The staging shelf

The **shelf** (the tray at the bottom, toggled from the header) collects files
from different folders and tabs: drop files on it, or use **Add to Shelf** in the
context menu. Each chip can be removed, and the whole batch is dragged out at
once into a destination folder, a USB drive, or a browser upload. It holds
references, not copies, so nothing moves until you drop it somewhere; a chip
whose file has since been moved or deleted disappears on its own.

A drag that starts in this window **moves** the files, the way Finder does on
one disk; a drag that lands on another filesystem, or that comes from another
application (a browser download, an archive tool), **copies** them instead. A
move is undoable with **Ctrl+Z**, so a drop into the wrong folder is one key
away from coming home.

## Archives

Right-click an archive (`.zip`, `.tar.gz` / `.tgz`, `.tar.xz` / `.txz`) for
**Extract Here** - it unpacks beside the archive and never overwrites what is
already there. Right-click any selection for **Compress to .zip / .tar.gz /
.tar.xz**; the archive lands in the current folder under a free name
(`notes.zip`, then `notes (2).zip`). Both run on the system `tar` and `zip`, so
there is no extra library in memory, and both use the same progress bar as a
copy.

## Undo

**Ctrl+Z** reverses the most recent reversible action - a rename, a move (drag,
cut-paste or drop), a move to Trash, a copy, or a new folder. A rename goes back
to its old name, a move comes home, a copy is removed, and a trashed file is
restored from Trash. A permanent delete is not undoable, which is why it asks
first. The banner says **Ctrl+Z to undo** whenever the last action can be
reversed.

## Quick Properties

**Alt+Enter** or **Ctrl+I** opens a compact inspector for the selection: its
kind, exact size (bytes and a human figure, plus a folder's item count), where
it lives, when it changed, its permissions in `rwx` and octal, its owner and
group, its MIME type, a symlink's target, and a **Compute SHA-256** button for a
file. A regular file also gets a one-click **Make executable / Remove
executable** toggle, so a downloaded script becomes runnable without a terminal.

## Places

Right-click a pinned folder in **PLACES** to **Unpin from Places** (or open it in
a new tab). To pin a folder, drag it onto the **PLACES** heading; the header
lights up while you hover. The list is stored in
`~/.config/hayato/filer-bookmarks.json`.

## Spring-loaded tabs and folders

Hold a drag over a tab or a folder for about half a second and it opens
(switches tab, or enters the folder) so the drop can land deeper than one level,
without letting go - the Finder's spring-loading.

## It opens where you left it

The window remembers the grid or list view, the sort field and order, the hidden
files setting, the compact sidebar, and the window size in
`~/.config/hayato/filer-state.json`. Delete that file to go back to the defaults.
