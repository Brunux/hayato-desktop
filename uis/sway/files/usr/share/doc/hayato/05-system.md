# The system

## The disk, by boot backend

Hayato uses pluggable boot backends selected by `/etc/hayato/deviceinfo`
(`deviceinfo_boot_type`), rather than assuming ChromeOS partitions on every board:

* **`depthcharge` (Google Hayato / `google-asurada` & Google Kukui / `google-kukui`)** — three partitions matching ChromeOS firmware:

| Partition | Size | What |
|---|---|---|
| `p1` `pmOS_kernel` | 128 MiB | the **signed FIT** ("kpart", `lz4`-compressed for fast firmware boot) the firmware loads: kernel, initramfs, and DTBs in one file |
| `p2` `pmOS_boot` | 512 MiB | ext2, mounted on `/boot`: kernel, initramfs, and DTBs unpacked |
| `p3` `pmOS_root` | the rest | ext4 root filesystem |

  ChromeOS firmware does not read `/boot` directly; it loads `p1`. When you
  update a kernel on Hayato, finish by refreshing the active kernel partition:

  ```sh
  CONFIRM=yes /usr/libexec/hayato/install-kpart.sh
  ```

  (Or `sudo CONFIRM=yes /usr/libexec/hayato/install-boot.sh`, which dispatches to
  it.) `install-kpart.sh` finds the running partition via `kern_guid` in
  `/proc/cmdline`, builds a fresh kpart, verifies it is dev-signed, backs up the
  previous partition image to `~/.cache/debian-aarch64/work/`, and writes it
  when `CONFIRM=yes` is set.

* **`uefi` (`arm64-uefi`)** — two GPT partitions: `p1` `hayato_efi` (256 MiB,
  FAT32 labeled `HAYATO_EFI`, `systemd-boot` EFI System Partition) and `p2`
  `hayato_root` (ext4 labeled `hayato_root`).
* **`extlinux` (`arm64-extlinux`)** — two GPT partitions starting after the
  16 MiB U-Boot reservation: `p1` `hayato_boot` (512 MiB, ext2 labeled
  `hayato_boot`, `/boot/extlinux/extlinux.conf`) and `p2` `hayato_root` (ext4
  labeled `hayato_root`).

On `depthcharge`, `uefi`, and `extlinux`, `/etc/kernel/postinst.d/zz-hayato-boot`
and `/etc/initramfs/post-update.d/zz-hayato-boot` invoke
`/usr/libexec/hayato/install-boot.sh` automatically on `apt upgrade` to sync
DTBs and refresh either the signed ChromeOS `kpart` or the boot entry.

## Boot splash

Booting displays the minimalist pixel-art **Hayato** Plymouth splash theme
(`/usr/share/plymouth/themes/hayato/`) on a solid `#1a1b26` Tokyo Night canvas:
each character scrambles briefly through cyan pixel glyphs before locking into
crisp pixel-grid `Hayato`, followed by a square-wave blinking orange block
cursor, a 12-dot cyan pixel scanner row, and subtle periodic glitch pulses
until `greetd` takes over seamlessly — where `hayato-greeter` continues the
same pixel-hacker `Hayato █` wordmark and dot scanner styled in the active
greeter theme. It is enabled via `splash` and
`plymouth.ignore-serial-consoles` (so ARM64 device-tree UARTs like `ttyS0` do
not suppress the graphical splash) in `/etc/hayato/kernel-cmdline.conf` and
`plymouth-set-default-theme hayato`.

## The kernel

Generic targets (`arm64-uefi`, `arm64-extlinux`) use Debian `forky`'s packaged
`linux-image-arm64` (`debian-arm64`), while custom mainline ARM64 boards can
assemble `devices/kernels/arm64-mainline/` from `base.config` plus SoC
fragments.

On **Google Hayato** (`google-asurada`) and **Google Kukui** (`google-kukui`, MediaTek MT8183),
Hayato keeps Linux **7.2.7-mt81**,
built from `devices/kernels/mediatek-mt81/` with clang and lld. It is not a
Debian kernel: the MediaTek display, MMC, panfrost (the GPU) and ext4 are built
in, and the configuration carries this machine's tuning:

* **MGLRU** enabled by default - page reclaim by generation instead of a full
  scan, which is what 4 GB under pressure wants;
* **schedutil** as the CPU governor (`cat
  /sys/devices/system/cpu/cpufreq/policy*/scaling_governor`), instead of
  `ondemand`, which pinned both clusters at maximum;
* **1000 Hz** timers, full preemption;
* transparent huge pages set to `madvise` on the command line.

Memory: `zram` provides a **1.9 GB** compressed swap device - half of RAM, and the half
is the decision, not the package's default. The pages it holds are compressed and cost
about a quarter of the RAM their size suggests (measured here: 427 MB of pages in
98 MB, ~4.4:1 ratio), so 1.9 GB of swap spends ~0.44 GB of memory and this machine's
measured heavy load (1.4 GB) fits inside it. The other half is the session's on
purpose: data that does not compress - a video, an archive, an encrypted blob - costs
one byte of RAM per byte of swap, so a device sized to all of RAM could hold 3.7 GB
resident before a single page reached the card, and that is memory the machine no
longer has. Past the half, the overflow goes to the auxiliary swap on a card
(priority 10, below zram's 100). Swapping itself is cheap because
`vm.page-cluster = 0` decompresses one 4 KB page instead of eight (8-page readahead
blew out the 32 KB L1 data cache on the Cortex-A55 LITTLE cores),
`vm.vfs_cache_pressure = 100` reclaims clean directory cache instead of evicting
process memory, and `vm.watermark_boost_factor = 0` stops premature kswapd churn.
Storage interrupts (MMC) are routed to CPU2/3 so CPU0 (the input/EC core) is never
choked by I/O traffic. `systemd-oomd` kills a memory hog at 50 % pressure instead of
letting the machine freeze. `swapon --show`, `zramctl` and
`cat /sys/block/zram0/mm_stat` show what is in use.

## When you leave it alone

The session sleeps in three steps, and each one exists because the previous one
left something on:

| After | What happens |
|---|---|
| 2½ minutes | the Hayato idle screen starts (one foot window, 30 fps, nothing else) |
| 5 minutes | the session locks (`sway-lock`, the same lock every other path uses) |
| 6 minutes | the panel goes dark (`swaymsg "output * power off"`, i.e. DPMS) |

Those are the defaults. **Settings → Power & Idle → Idle & lock** writes
`~/.config/hayato/idle.conf`; `~/.config/bin/hayato-idle` reads it at login
(`~/.config/sway/config.d/36-idle.conf`) and replaces the system chain with the
same three steps at your times. `0` means never and omits that step.

Any input ends the idle screen and brings the panel back: a key, a moving mouse
or trackpad, a tap on the touchscreen. swayidle's `resume` command turns the
panel on again, and `after-resume` does the same after a suspend, which is a
different event and would otherwise wake to a dark screen. The lock answers
typing only - `swaylock` ignores the pointer - so at five minutes a mouse move
only lights the screen and the password ends the lock. The
idle chain is `/etc/sway/config.d/23-hayato-screensaver.conf`; `check-system.sh`
reads it and fails when a step or either wake line is missing, when the pointer
can no longer end the idle screen, and when the 150 s command stops returning at
once (swayidle waits for it, and the lock and the panel-off timer would never
arrive).

A **fullscreen video or presentation** in Firefox or `papers`, and a video in
**mpv**, are exempt: they ask the compositor to inhibit the idle timer while
they are being watched, so a film is not interrupted and the screen stays on.
The automatic half is `/etc/sway/config.d/32-idle-inhibit.conf`; the manual half
is the **coffee cup on the bar**, in the workspace accent while the screen is
kept awake and dim when it may sleep. It is waybar's own module and starts no
process.

## The first launch

Two caches are built once, inside the image, instead of on your first session:

* **fontconfig's system cache** (`/var/cache/fontconfig`) - without it the first
  terminal to draw a glyph walks every font directory;
* **the icon theme's index** (`/usr/share/icons/Hayato/icon-theme.cache`) -
  without it the first launcher or notification lookup scans 220 files before it shows a picture.

Both are rebuilt by `core/scripts/rootfs-setup.sh` at build time. `check-system.sh`
warns when either is missing - a rebuilt theme that did not refresh its index is
the silent case, because the launcher still opens, just later.

## Notifications

A notification is an eye-candy card at the top right, under the bar, rendered by
`hayato-notify` over whatever is open — including over fullscreen applications for
critical alerts. Its anti-aliased border and glowing dot indicate urgency: low
(muted grey), normal (accent cyan/theme border), or critical (warning red).
Normal and low notifications expire automatically; critical cards remain until
dismissed.

**Interaction:**
* **Left-click on card body:** when the card names a written file (a screenshot, an annotation, a saved recording), opens that file's folder in Files; otherwise invokes the default action (if defined) or dismisses.
* **Left-click on action pill:** triggers that specific action and signals the sender via D-Bus. A file card always carries a **Files** pill.
* **Alt+Comma:** acts on the newest card without taking focus: its default action, its file's folder, or dismiss.
* **Alt+Shift+Comma:** dismisses the newest card. **Alt+Ctrl+Comma** clears the visible stack.
* **Left-click on top-right '✕':** closes the notification.
* **Right-click on card:** dismisses the card.
* **Middle-click on card:** dismisses all notifications from that application.

Cards from the same sender sit together, and identical repeats collapse into one card with a **×N** badge. The history still records every repeat.

Each card carries a high-resolution icon from the Hayato theme or a single-advance Nerd
Font glyph fallback, rendered in the active theme's palette matching the Waybar hover tooltip
box (`surface` background, `dim` 1px border, 6px corner radius, and `accent` highlights).
Notifications also render level bars for progress hints (`value:0..100`), used by volume/brightness
keys (`osd`) and slow background tasks. Volume adjustments clamp strictly at 100% (matching
hardware brightness controls) to prevent software audio distortion.

`hayato-notify` logs notifications into structured JSONL at
`$XDG_STATE_HOME/hayato/notify/history.jsonl`. **Alt+Ctrl+N** opens the history
viewer (`hayato menu notify`): Enter copies a notification to the clipboard,
"Restore the last one" re-issues the most recent toast, and Do Not Disturb toggles
quiet mode without suppressing critical system alerts. Secrets never appear in
notification bodies: tokens and passwords go directly to the clipboard.

## Health and backups

```sh
check-system.sh                 # OK / WARN / FAIL, one line per check
```

Every check corresponds to a bug this port has already hit, so a `FAIL` is a
regression, not a theory. The image ships the script as
`/usr/local/sbin/check-system.sh` too.

The session half of the report is worth knowing about: it reads the **idle chain**
(150 s screen, 300 s lock, 360 s panel off, both wake lines and the lock before
sleep), the **resident set** (a declined daemon such as `udiskie`, `swaync` or an
indexer is a `FAIL`; a user unit outside PipeWire/WirePlumber/portals/gvfs is a
`WARN`), the **two caches** above, the **non-Latin font**, the **GTK4 files**, the
**overlay** behind the volume and brightness keys, and the **`ip`/`ping`/`ss`**
trio. A `WARN` there usually names a missing piece of the look rather than a
broken machine.

Backups are made of the *parts* a disk cannot be regenerated from:

```sh
sudo /usr/libexec/hayato/backup-system.sh /media/you/BACKUP
sudo /usr/libexec/hayato/restore-system.sh /media/you/BACKUP /dev/mmcblk0
```

The kernel partition, the partition table, `/boot` and the root filesystem go
into tars (ACLs and xattrs preserved) plus a `README.md` with the restore steps
and a `SHA256SUMS`. `restore-system.sh` wants a disk of the same size and
`CONFIRM=yes`, and refuses to run from the disk it would overwrite.

## Remote login (ssh)

`openssh-server` is installed and `ssh.service` is **enabled on every boot**, on
purpose: it is the recovery path. A machine whose session will not start, or
whose screen is not usable, can still be reached from another computer, and a
card that installs the eMMC is the same system that can rebuild its own kernel
partition - over ssh if that is the only way in.

That also means the machine answers a login prompt on whatever network it joins.
The build's default credentials are **`brunux` / `debian`** with passwordless
`sudo`; change the password on the first day:

```sh
passwd
```

`check-system.sh` prints the fact (`---- remote login is enabled (ssh, …)`) so the
state is never a surprise, and to turn it off:

```sh
sudo systemctl disable --now ssh
```


## Time zone

The **Time zone** menu (Settings → *Time zone*, and the root menu) lists every zone tzdata knows (400-odd rows, filtered by typing). The current zone is marked. The network-time switch at the end toggles `systemd-timesyncd` (which is in packages/base.list for this reason).

Picking a zone runs `timedatectl set-timezone` and restarts waybar so the bar clock updates (glibc caches the zone at the first `localtime()` call in a process). The notification shows the new local time so the change is visible even before the bar restarts.

The `--status` subcommand returns the current zone and NTP state.


## DNS resolvers

The **DNS** menu (Settings → *DNS*, and the root menu) points the active connection at a resolver or back at DHCP. NetworkManager keeps `/etc/resolv.conf`, so the switch is two nmcli properties on the connection that is up: `ipv4.dns` and `ipv4.ignore-auto-dns`, followed by bringing that connection up again so the change is applied at once.

The re-activation is why this says something before it acts: a Wi-Fi connection that comes back up costs a second or two, and a menu that is already closed while the network blinks is indistinguishable from a bug. The `busy` card stays until the connection answers again and then becomes the read-back from NetworkManager, not the value that was asked for.

The menu offers Automatic (DHCP), Cloudflare, Google, Quad9, AdGuard, and a custom entry. It does not touch IPv6 DNS. The card registers no stop command — killing `nmcli connection up` is safe but leaves the profile saying one thing while resolv.conf holds another.

The `--status` subcommand returns the current servers and ignore-auto-dns state.

## Hotel and airport Wi-Fi

A network that wants you to accept its terms before it lets you online is a
**captive portal**. NetworkManager reports it as `portal` or `limited`; the
**Wi-Fi** menu (`hayato menu net`) then starts with **Sign in to this network**,
and Enter opens the page in Firefox. The page is found by asking NetworkManager's
own connectivity endpoint where it redirects; only `http` and `https` URLs are
opened. Settings → Network shows the same row while a sign-in is pending. After
the page accepts you, the row disappears on the next scan or connection.

`hayato menu net --portal` prints the URL (or `none`) for scripts and checks.

## Looking at the screen from another machine

`wayvnc` serves this session as VNC, so a screen that is misbehaving can be
looked at from a bigger machine — the other half of what ssh gives you, which is
a shell but not the picture. A rotated screen, a window that will not draw, a
lock screen that came up wrong: all of those are one glance away instead of one
photograph.

**Settings** (Alt+Ctrl+O) → **Share this screen** starts it and says where it is
serving; the same list is in the root menu (Alt+Space), and it is where **Stop
sharing** and **Copy the tunnel command** live. While it is on, the bar shows an
eye for read-only sharing or a keyboard for remote control (right click stops
either).

It listens on **127.0.0.1:5900 and nothing else**. There are two starts, and the
first is the safe one:

* **Share this screen** (the default) is **read-only**: `wayvnc
  --disable-input` creates no virtual keyboard and no virtual pointer, so the
  viewer cannot type, click or move the pointer here.
* **Allow remote control** starts the same server without that flag, so the
  viewer's keyboard and pointer reach this session. It is still
  **127.0.0.1:5900 and nothing else**, reached through the same ssh tunnel, so
  the login that opens the tunnel is the authentication — and now it is the
  authentication for input too.

The loopback address is a decision, not a limitation:

* wayvnc 0.10 authenticates only through `~/.config/wayvnc/config`, where
  `enable_auth` wants a certificate, a private key and a password. This image
  ships no such file, so a server on the network address would be an
  unauthenticated desktop on every network the machine joins.
* the loopback address is reached through the **ssh login the image already
  answers** (`openssh-server` is installed and `ssh.service` enabled on every
  boot), so the tunnel *is* the authentication — using the part that is already
  configured, tested and documented.
* read-only stays the default because "see what is on the screen" never needs to
  touch it. **Allow remote control** is for the case where a window has to be
  clicked from the other machine; while it is on, the bar shows a **keyboard**
  instead of the eye and the tooltip says so. **Switch to view only** (or
  **Switch to remote control**) stops and restarts the server in the other
  mode, and **Stop sharing** ends either one.

From the other machine:

```sh
ssh -L 5900:127.0.0.1:5900 brunux@hayato
```

and then point a VNC viewer at `localhost:5900`: Remmina, TigerVNC's
`vncviewer`, `gvncviewer`, or on macOS the built-in Screen Sharing app
(`vnc://localhost:5900`). The **Copy the tunnel command** row copies that line
with this machine's user and host name already in it. Keep the ssh session open
for as long as you are watching.

A few things follow from the rules above and are worth knowing before debugging
them:

* **No audio.** VNC carries pixels; the sound stays on this machine.
* **Remote control is input.** Whatever has focus receives it — a terminal, a
  browser, a password prompt. Turn it on only while you are the one typing, and
  stop sharing when you are done.
* **The mode is the process.** `status` and the bar read
  `/proc/<pid>/cmdline`, so a stale file cannot claim read-only while the server
  accepts input; switching modes restarts the server rather than editing a flag.
* **Nothing runs until the row is used**, and nothing stays after **Stop
  sharing**. There is no unit and no autostart: `wayvnc` is one process started
  by the menu, and the bar's icon is a state file being read (one `sh` every ten
  seconds), not a daemon.
* **A session that ends takes the state with it** (it lives in
  `$XDG_RUNTIME_DIR/hayato-vnc`), so a logout cannot leave a stale "on".
* **Port 5900 is the only port.** If something else already listens there, the
  menu says so — with wayvnc's own message — instead of starting half a server.
* `~/.config/bin/hayato-vnc status` prints the state in one line
  (`localhost:5900 · read-only`, `localhost:5900 · remote control`, or `off`),
  which is the quickest answer to "is this machine being watched right now".

## Suspend and hibernate

Suspend-to-RAM works and resumes cleanly. Two paths lead to it, and they are
different on purpose:

* **The Lock key** is `KEY_SLEEP`, and that is exactly the key systemd-logind
  turns into a suspend on its own. It needs no polkit, no session and no helper,
  which is why it always works. The same key runs `~/.config/bin/sway-lock`, so
  the screen is locked a fraction of a second before the machine sleeps.
* `sway-suspend` (Alt+Shift+Escape locks first, Alt+Ctrl+Escape suspends
  directly) goes through `loginctl suspend`, a D-Bus method guarded by polkit. It
  has a fallback chain and logs to `~/.cache/sway-suspend.log`, so if the session
  is in a state logind does not consider active, it still works.

`sway-lock` is the lock every path goes through, including the 300 s idle timer:
it stops the idle screen first, waits (up to 4 s before a suspend) for
`swaylock`'s own readiness signal, and puts the session's wallpaper behind the
lock instead of a flat colour.

**Hibernate** is offered when it can actually resume:

* a swap on disk (not zram) big enough for the image - the kernel targets 2/5 of
  RAM, so 2 GB is the practical minimum here;
* `resume=` on the kernel command line, so the next boot restores the image
  instead of starting fresh. Because `resume=` is resolved through udev, a
  **label** is enough: a swap partition labelled `hayato_swap` plus
  `resume=LABEL=hayato_swap` on the command line.

The **Auxiliary swap** row in Places (and in the root menu) writes that partition
and the `/etc/fstab` line that activates it on every boot, with priority 10 so
zram keeps being used first. If the card already carries a swap area - whatever
its label, including one written by an older release - the row offers to **adopt
it**: the partition is relabelled `hayato_swap`, the `/etc/fstab` line is written
and it is activated, with nothing erased. Erasing and repartitioning stays a
separate choice, and when no swap area is found the row offers to create one from
the whole card or a fixed 2/4/8 GiB slice. It does not add `resume=`: that is a
kernel command line change and needs the kernel partition rebuilt. The image
ships the checkout it was built from, so this is done once, on the machine:

```sh
CONFIRM=yes RESUME=yes /usr/libexec/hayato/install-kpart.sh
```

`install-kpart.sh` asks for `CONFIRM=yes` because it rewrites the partition this
system booted from, and it keeps a copy of what it replaced. It also **carries a
`resume=` the machine already has across a kernel update**, because dropping it is
silent: the machine keeps hibernating and then boots cold with the session gone.
`RESUME=yes` is how it gets added the first time, and `RESUME=no` is how a machine
whose swap card is gone gets rid of it. Nothing is in the image's own command
line: a card with no `hayato_swap` partition boots exactly as it did before,
because only a machine that asked for `resume=` waits for one.

`sway-hibernate` (in the root menu, and as a launcher entry) checks both and says
which one is missing. It never hibernates into a swap the machine cannot resume
from: a `resume=` that names a device this boot cannot find (a relabelled or
removed swap card) is refused too, because the image would be written and the
next boot would come up cold. If those conditions pass and `systemctl` still says
no, its own words become the card instead of a journal line nobody reads.
`~/.config/bin/sway-hibernate --status` prints the conditions in one block
(`kernel=`, `swap=`, `swap_kb=`, `need_kb=`, `resume=`, `resume_broken=`,
`hibernate=`) and does nothing else: it is the quickest answer to "why does it
refuse?", and what `check-system.sh` asks so that the size it reads is the
partition's size and not how much of it is in use.

## The microSD and the firmware

The internal eMMC and the microSD are written with matching layouts
(`core/scripts/write-sd.sh`, `hayato installer`, or fallback `/usr/libexec/hayato/install-emmc.sh`), so a card is a full rescue
system: the same three partitions, the same signed kpart, the same session. The
stock firmware's developer-mode menu picks **internal disk** / **Ctrl+D** (the eMMC) or
**external disk** / **Ctrl+U** (the card/USB), so a broken install on the eMMC is recoverable by
booting the card and running the installer again.

## Install Hayato (The Pure-Rust TUI & Headless Installer)

Installing Hayato onto internal storage (`/dev/mmcblk0`, `/dev/nvme0n1`, etc.) is driven
natively in pure Rust by `hayato installer` across **all supported boot architectures**
(`depthcharge` on `google-asurada` / `google-kukui` Chromebooks, `uefi` with `systemd-boot`, and `extlinux` on U-Boot SBCs):

```sh
hayato installer                                        # interactive full-screen wizard in foot
hayato installer --check                                # headless verification of boot type & disk safety
hayato installer --dry-run                              # non-destructive simulation of the full wizard
sudo hayato installer apply --clone --confirm           # headless live-clone install (auto-selected target)
```

For Google Hayato emergency recovery or shell debugging, `/usr/libexec/hayato/install-emmc.sh`
remains preserved intact as a manual fallback. `hayato-installer` resolves to `/usr/bin/hayato`,
keeping all key bindings, the root menu (*Install Hayato*), and `org.hayato.install.desktop`.

### The 6 Wizard Steps

1. **Welcome & Pre-Flight**:
   Reads `/etc/hayato/deviceinfo` without sourcing a shell (`depthcharge`, `uefi`, or `extlinux`),
   detects Devicetree or DMI SMBIOS hardware identity, evaluates target storage candidates, and
   verifies required tools per boot type. Evaluates the **strict safety invariant**: running from
   any partition on the target disk fails closed and prevents overwriting active storage.
2. **Localization, Timezone & Keyboard**:
   Searchable IANA timezone picker, system locale selector (`locale-gen` + `/etc/default/locale`),
   and keyboard layout selector (`/etc/default/keyboard` + `~/.config/sway/config.d/90-keyboard-layout.conf`)
   with a live keypress preview box.
3. **User Account & Host Identity**:
   Full name, POSIX username, masked password confirmation (passed over stdin pipe to `chpasswd`,
   never via argv or temp files), hostname, and SSH host-key regeneration toggle. When renaming
   the live account, desktop session configs and `.desktop` paths migrate automatically.
4. **Target Storage & Partition Review (Gated Confirmation)**:
   Displays the boot-specific GPT layout (`pmOS_kernel`/`pmOS_boot`/`pmOS_root` for `depthcharge`,
   `hayato_efi`/`hayato_root` for `uefi`, `hayato_boot`/`hayato_root` for `extlinux`), switches
   candidates when multiple non-active disks exist, starts cursor focus safely on `[ Cancel / Back ]`,
   and locks `[ Erase Disk & Install ]` until the target short name (e.g., `mmcblk0`, `nvme0n1`)
   or `DESTROY` is entered.
5. **TaskView Installation Progress**:
   Spawns `sudo -n setsid systemd-inhibit ... /usr/bin/hayato installer apply --stdin --state-dir /run/hayato-install`,
   streaming `rsync --info=progress2` progress, pausing `swayidle`, and enforcing the `first_write`
   cancellation lock once GPT partitioning begins (`Esc` detaches while the worker finishes in background).
6. **Completion**:
   Writes `/etc/hayato/installation` last, verifies root ext4 cleanly with `e2fsck -f -n`,
   displays boot instructions tailored to Depthcharge, UEFI (`systemd-boot`), or `extlinux`, and
   offers `[ Reboot System Now ]` (running the detached session reboot chain with `sudo -n systemctl reboot`
   fallback) or `[ Stay in Live Session ]`.

## Kernel Updates & In-Place `.deb` Upgrades

* **`google-asurada` & `google-kukui` (`depthcharge`)**: use the shared `mediatek-mt81` (`7.2.7`) kernel packaged as `linux-image-hayato-mediatek-mt81` with
  Hayato tuning (`HZ=1000`, MGLRU, `schedutil`), QCA6174/MT7921s/SCP firmware (`google-kukui`), and
  board ALSA UCM2/keyd overlays (`hayato-device-<board>`). Whenever `apt upgrade` installs a new kernel or rebuilds the initramfs, `/etc/kernel/postinst.d/zz-hayato-boot` and `/etc/initramfs/post-update.d/zz-hayato-boot` automatically invoke `CONFIRM=yes TARGET_KERNEL_RELEASE=<ver> /usr/libexec/hayato/install-boot.sh` (`install-kpart.sh`) to re-sign and flash the active ChromeOS `kern_guid` partition.
* **`arm64-uefi` & `arm64-extlinux` (`debian-arm64`)**: use Debian `forky`'s packaged
  `linux-image-arm64` kernel plus broad multi-vendor firmware. Whenever `apt upgrade` updates the
  kernel, `/etc/kernel/postinst.d/zz-hayato-boot` automatically syncs `/usr/lib/linux-image-*`
  DTBs into `/boot/dtbs/` and refreshes `/boot/efi/loader/entries/hayato.conf` or
  `/boot/extlinux/extlinux.conf` via `/usr/libexec/hayato/install-boot.sh`.

```sh
sudo hayato-apt enable                       # enable Cloudsmith APT repository (once)
sudo apt update && sudo apt full-upgrade     # in-place upgrade of Debian + Hayato .debs
sudo hayato-apt upgrade-from-github          # or upgrade directly from GitHub Release .debs
check-system.sh
```

Hayato ships its framework (`hayato`), core runtime (`hayato-core`), board
definitions (`hayato-device-<board>`), custom kernels
(`linux-image-hayato-<flavor>`), compositor (`swayfx`, `libscenefx-*`),
workspace renamer (`workstyle`), and game stack (`retroarch`,
`hayato-libretro-fbneo`, `hayato-libretro-mupen64plus-next`) as `.deb` packages.
Upgrading `hayato` refreshes `/usr/bin/hayato*`, system configs, user helper
scripts in `~/.config/bin/`, and re-renders the active theme headlessly while
preserving personal credentials (`~/.config/opencode/{cli.json,service.json}`)
and user state.
