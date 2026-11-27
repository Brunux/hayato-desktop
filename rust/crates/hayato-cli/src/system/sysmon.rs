//! Stateless system telemetry probe for Waybar (`custom/sysmon`, `hayato sysmon --bar`).
//!
//! Runs once per Waybar poll interval with zero background daemons, caching only
//! the previous `/proc/stat` CPU sample in `$XDG_RUNTIME_DIR/hayato/sysmon.stat`
//! to compute live delta CPU usage without blocking.

use std::fmt::Write as _;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

const ICON_SYSMON: &str = "<span font_desc='JetBrains Mono 13'>\u{f4bc}</span>";
const WARN_PCT: u8 = 90;
const CRIT_PCT: u8 = 95;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuSample {
    pub total: u64,
    pub idle: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskUsage {
    pub mount_label: String,
    pub used_bytes: u64,
    pub total_bytes: u64,
}

impl DiskUsage {
    pub fn pct(&self) -> u8 {
        pct_of(self.used_bytes, self.total_bytes)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SysSnapshot {
    pub cpu_pct: Option<u8>,
    pub cpu_sustained_high: bool,
    pub cores: usize,
    pub load_1m: Option<f32>,
    pub temp_c: Option<i32>,
    pub mem_used_kb: u64,
    pub mem_total_kb: u64,
    pub swap_used_kb: u64,
    pub swap_total_kb: u64,
    pub root_disk: Option<DiskUsage>,
    pub home_disk: Option<DiskUsage>,
    pub same_root_home: bool,
    pub uptime_secs: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pressure {
    Ok,
    Warn,
    Critical,
}

impl Pressure {
    fn class(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Critical => "critical",
        }
    }
}

/// Entrypoint for direct `hayato sysmon` invocations.
pub fn run(args: &[String]) -> i32 {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("hayato sysmon --bar: emit Waybar JSON telemetry for custom/sysmon");
        return crate::EXIT_OK;
    }
    println!("{}", bar_json());
    crate::EXIT_OK
}

/// Collect live telemetry from `/proc`, `/sys`, and `statvfs` and emit Waybar JSON.
pub fn bar_json() -> String {
    let snap = collect_snapshot();
    bar_json_for(&snap)
}

fn collect_snapshot() -> SysSnapshot {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let load_1m = std::fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|s| parse_proc_loadavg(&s));

    let cur_cpu = std::fs::read_to_string("/proc/stat")
        .ok()
        .and_then(|s| parse_proc_stat(&s));
    let (cpu_pct, cpu_sustained_high) = compute_cpu_state(cur_cpu, load_1m, cores);

    let (mem_used_kb, mem_total_kb, swap_used_kb, swap_total_kb) =
        std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|s| parse_proc_meminfo(&s))
            .unwrap_or((0, 0, 0, 0));

    let uptime_secs = std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| parse_proc_uptime(&s))
        .unwrap_or(0);

    let temp_c = read_thermal_c();

    let home_dir = std::env::var_os("HOME").map(PathBuf::from);
    let same_root_home = match (&home_dir, std::fs::metadata("/")) {
        (Some(home), Ok(root_meta)) => std::fs::metadata(home)
            .map(|home_meta| home_meta.dev() == root_meta.dev())
            .unwrap_or(true),
        _ => true,
    };

    let root_disk = read_disk_usage(Path::new("/"), if same_root_home { "/ ~" } else { "/" });
    let home_disk = if same_root_home {
        None
    } else {
        home_dir
            .as_deref()
            .and_then(|home| read_disk_usage(home, "~"))
    };

    SysSnapshot {
        cpu_pct,
        cpu_sustained_high,
        cores,
        load_1m,
        temp_c,
        mem_used_kb,
        mem_total_kb,
        swap_used_kb,
        swap_total_kb,
        root_disk,
        home_disk,
        same_root_home,
        uptime_secs,
    }
}

fn state_path() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|s| !s.is_empty())
        .map(|dir| PathBuf::from(dir).join("hayato/sysmon.stat"))
}

fn compute_cpu_state(
    cur: Option<CpuSample>,
    load_1m: Option<f32>,
    cores: usize,
) -> (Option<u8>, bool) {
    let Some(cur) = cur else {
        let est = estimate_cpu_from_load(load_1m, cores);
        return (est, false);
    };
    let path = state_path();
    let (prev, prev_high) = path
        .as_deref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| parse_state(&s))
        .unwrap_or((None, false));

    let pct = cpu_pct_from_delta(prev, cur).or_else(|| estimate_cpu_from_load(load_1m, cores));
    let is_high = pct.is_some_and(|v| v >= WARN_PCT);
    let sustained = is_high && prev_high;

    if let Some(p) = path {
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let body = format!("{} {} {}\n", cur.total, cur.idle, u8::from(is_high));
        let _ = std::fs::write(&p, body);
    }
    (pct, sustained)
}

fn parse_state(raw: &str) -> Option<(Option<CpuSample>, bool)> {
    let mut parts = raw.split_whitespace();
    let total: u64 = parts.next()?.parse().ok()?;
    let idle: u64 = parts.next()?.parse().ok()?;
    let prev_high = parts.next().and_then(|v| v.parse::<u8>().ok()).unwrap_or(0) != 0;
    Some((Some(CpuSample { total, idle }), prev_high))
}

fn estimate_cpu_from_load(load_1m: Option<f32>, cores: usize) -> Option<u8> {
    let load = load_1m?;
    let c = cores.max(1) as f32;
    Some(((load / c) * 100.0).clamp(0.0, 100.0).round() as u8)
}

/// Compute delta CPU busy percentage from two `/proc/stat` samples.
pub fn cpu_pct_from_delta(prev: Option<CpuSample>, cur: CpuSample) -> Option<u8> {
    let prev = prev?;
    let total_delta = cur.total.checked_sub(prev.total)?;
    if total_delta == 0 {
        return None;
    }
    let idle_delta = cur.idle.saturating_sub(prev.idle).min(total_delta);
    let busy = total_delta - idle_delta;
    Some(
        ((busy as f64 / total_delta as f64) * 100.0)
            .clamp(0.0, 100.0)
            .round() as u8,
    )
}

/// Parse the aggregate `cpu` line from `/proc/stat`.
pub fn parse_proc_stat(raw: &str) -> Option<CpuSample> {
    let line = raw.lines().find(|l| l.starts_with("cpu "))?;
    let vals: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|v| v.parse::<u64>().ok())
        .collect();
    if vals.len() < 4 {
        return None;
    }
    let idle = vals[3].saturating_add(vals.get(4).copied().unwrap_or(0));
    let total = vals.iter().copied().sum();
    Some(CpuSample { total, idle })
}

/// Parse `(mem_used_kb, mem_total_kb, swap_used_kb, swap_total_kb)` from `/proc/meminfo`.
pub fn parse_proc_meminfo(raw: &str) -> Option<(u64, u64, u64, u64)> {
    let mut mem_total = None;
    let mut mem_avail = None;
    let mut mem_free = None;
    let mut buffers = 0u64;
    let mut cached = 0u64;
    let mut swap_total = 0u64;
    let mut swap_free = 0u64;

    for line in raw.lines() {
        let mut parts = line.split_whitespace();
        let Some(key) = parts.next() else { continue };
        let Some(val) = parts.next().and_then(|v| v.parse::<u64>().ok()) else {
            continue;
        };
        match key {
            "MemTotal:" => mem_total = Some(val),
            "MemAvailable:" => mem_avail = Some(val),
            "MemFree:" => mem_free = Some(val),
            "Buffers:" => buffers = val,
            "Cached:" => cached = val,
            "SwapTotal:" => swap_total = val,
            "SwapFree:" => swap_free = val,
            _ => {}
        }
    }
    let total = mem_total?;
    let avail = mem_avail.unwrap_or_else(|| mem_free.unwrap_or(0) + buffers + cached);
    let mem_used = total.saturating_sub(avail);
    let swap_used = swap_total.saturating_sub(swap_free);
    Some((mem_used, total, swap_used, swap_total))
}

/// Parse 1-minute load average from `/proc/loadavg`.
pub fn parse_proc_loadavg(raw: &str) -> Option<f32> {
    raw.split_whitespace().next()?.parse().ok()
}

/// Parse uptime in whole seconds from `/proc/uptime`.
pub fn parse_proc_uptime(raw: &str) -> Option<u64> {
    let first = raw.split_whitespace().next()?;
    first.split('.').next()?.parse().ok()
}

fn read_thermal_c() -> Option<i32> {
    let entries = std::fs::read_dir("/sys/class/thermal").ok()?;
    let mut preferred = None;
    let mut fallback = None;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with("thermal_zone") {
            continue;
        }
        let Ok(raw_temp) = std::fs::read_to_string(path.join("temp")) else {
            continue;
        };
        let Ok(milli) = raw_temp.trim().parse::<i32>() else {
            continue;
        };
        if !(1_000..=130_000).contains(&milli) {
            continue;
        }
        let c = (milli + 500) / 1_000;
        let zone_type = std::fs::read_to_string(path.join("type"))
            .unwrap_or_default()
            .to_ascii_lowercase();
        if zone_type.contains("cpu")
            || zone_type.contains("soc")
            || zone_type.contains("pkg")
            || zone_type.contains("tsen")
        {
            preferred = Some(preferred.map_or(c, |p: i32| p.max(c)));
        } else {
            fallback = Some(fallback.map_or(c, |p: i32| p.max(c)));
        }
    }
    preferred.or(fallback)
}

fn read_disk_usage(path: &Path, label: &str) -> Option<DiskUsage> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let cpath = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: valid NUL-terminated path and aligned MaybeUninit buffer.
    let rc = unsafe { libc::statvfs(cpath.as_ptr(), stat.as_mut_ptr()) };
    if rc != 0 {
        return None;
    }
    // SAFETY: `statvfs` returned 0, so `stat` is initialized.
    let st = unsafe { stat.assume_init() };
    let frsize = if st.f_frsize > 0 {
        st.f_frsize
    } else {
        st.f_bsize
    };
    let blocks = st.f_blocks;
    let bfree = st.f_bfree;
    let bavail = st.f_bavail;
    let used_blocks = blocks.saturating_sub(bfree);
    let avail_plus_used = used_blocks.saturating_add(bavail);
    Some(DiskUsage {
        mount_label: label.to_string(),
        used_bytes: used_blocks.saturating_mul(frsize),
        total_bytes: avail_plus_used.saturating_mul(frsize),
    })
}

fn pct_of(used: u64, total: u64) -> u8 {
    if total == 0 {
        return 0;
    }
    ((used as f64 / total as f64) * 100.0)
        .clamp(0.0, 100.0)
        .round() as u8
}

fn format_bytes_iec(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    const TIB: f64 = GIB * 1024.0;
    let b = bytes as f64;
    if b >= TIB {
        format!("{:.1}T", b / TIB)
    } else if b >= GIB {
        format!("{:.1}G", b / GIB)
    } else if b >= MIB {
        format!("{:.0}M", b / MIB)
    } else {
        format!("{:.0}K", (b / KIB).max(0.0))
    }
}

fn format_uptime(secs: u64) -> String {
    let mins = secs / 60;
    let days = mins / (60 * 24);
    let hours = (mins / 60) % 24;
    let rem_mins = mins % 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {rem_mins}m")
    } else {
        format!("{rem_mins}m")
    }
}

fn block_gauge(pct: u8) -> String {
    let filled = ((u16::from(pct.min(100)) * 8 + 50) / 100) as usize;
    let empty = 8usize.saturating_sub(filled);
    let on = "\u{2588}".repeat(filled);
    if empty == 0 {
        on
    } else {
        let off = "\u{2591}".repeat(empty);
        format!("{on}<span alpha='35%'>{off}</span>")
    }
}

fn pango_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn pressure_assessment(snap: &SysSnapshot) -> (Pressure, &'static str) {
    let mem_pct = pct_of(snap.mem_used_kb, snap.mem_total_kb);
    let root_pct = snap.root_disk.as_ref().map_or(0, DiskUsage::pct);
    let home_pct = snap.home_disk.as_ref().map_or(0, DiskUsage::pct);
    let disk_pct = root_pct.max(home_pct);
    let cpu_pct = if snap.cpu_sustained_high {
        snap.cpu_pct.unwrap_or(0)
    } else {
        0
    };

    let worst = mem_pct.max(disk_pct).max(cpu_pct);
    if worst >= CRIT_PCT {
        let label = if disk_pct >= CRIT_PCT {
            "SYS // DISK CRIT"
        } else if mem_pct >= CRIT_PCT {
            "SYS // MEM CRIT"
        } else {
            "SYS // CPU CRIT"
        };
        (Pressure::Critical, label)
    } else if worst >= WARN_PCT {
        let label = if disk_pct >= WARN_PCT {
            "SYS // DISK HIGH"
        } else if mem_pct >= WARN_PCT {
            "SYS // MEM HIGH"
        } else {
            "SYS // HIGH LOAD"
        };
        (Pressure::Warn, label)
    } else {
        (Pressure::Ok, "SYS // LIVE")
    }
}

/// Format Waybar JSON (`text`, `class`, `tooltip`) for a [`SysSnapshot`].
pub fn bar_json_for(snap: &SysSnapshot) -> String {
    let (pressure, header_word) = pressure_assessment(snap);
    let up = format_uptime(snap.uptime_secs);
    let mut tooltip = format!(
        "<span alpha='50%'>╭─</span> <b>{header_word}</b> <span alpha='60%'>· up {up}</span>"
    );

    let cpu_pct = snap.cpu_pct.unwrap_or(0);
    let mut cpu_detail = format!("{}c", snap.cores.max(1));
    if let Some(load) = snap.load_1m {
        let _ = write!(cpu_detail, " · {load:.2}");
    }
    if let Some(temp) = snap.temp_c {
        let _ = write!(cpu_detail, " · {temp}°C");
    }
    let _ = write!(
        tooltip,
        "\n<span alpha='45%'>│</span> <span alpha='60%'>CPU</span>  {}  {:>3}%  <span alpha='65%'>({})</span>",
        block_gauge(cpu_pct),
        cpu_pct,
        pango_escape(&cpu_detail),
    );

    let mem_pct = pct_of(snap.mem_used_kb, snap.mem_total_kb);
    let mem_detail = format!(
        "{} / {}",
        format_bytes_iec(snap.mem_used_kb.saturating_mul(1024)),
        format_bytes_iec(snap.mem_total_kb.saturating_mul(1024)),
    );
    let _ = write!(
        tooltip,
        "\n<span alpha='45%'>│</span> <span alpha='60%'>MEM</span>  {}  {:>3}%  <span alpha='65%'>({})</span>",
        block_gauge(mem_pct),
        mem_pct,
        pango_escape(&mem_detail),
    );

    if snap.swap_total_kb > 0 && snap.swap_used_kb > 0 {
        let swap_pct = pct_of(snap.swap_used_kb, snap.swap_total_kb);
        let swap_detail = format!(
            "{} / {}",
            format_bytes_iec(snap.swap_used_kb.saturating_mul(1024)),
            format_bytes_iec(snap.swap_total_kb.saturating_mul(1024)),
        );
        let _ = write!(
            tooltip,
            "\n<span alpha='45%'>│</span> <span alpha='60%'>SWP</span>  {}  {:>3}%  <span alpha='65%'>({})</span>",
            block_gauge(swap_pct),
            swap_pct,
            pango_escape(&swap_detail),
        );
    }

    if let Some(root) = &snap.root_disk {
        let pct = root.pct();
        let detail = format!(
            "{} · {} / {}",
            pango_escape(&root.mount_label),
            format_bytes_iec(root.used_bytes),
            format_bytes_iec(root.total_bytes),
        );
        let _ = write!(
            tooltip,
            "\n<span alpha='45%'>│</span> <span alpha='60%'>DSK</span>  {}  {:>3}%  <span alpha='65%'>({detail})</span>",
            block_gauge(pct),
            pct,
        );
    }
    if let Some(home) = &snap.home_disk {
        let pct = home.pct();
        let detail = format!(
            "{} · {} / {}",
            pango_escape(&home.mount_label),
            format_bytes_iec(home.used_bytes),
            format_bytes_iec(home.total_bytes),
        );
        let _ = write!(
            tooltip,
            "\n<span alpha='45%'>│</span> <span alpha='60%'>HOM</span>  {}  {:>3}%  <span alpha='65%'>({detail})</span>",
            block_gauge(pct),
            pct,
        );
    }

    tooltip.push_str(
        "\n<span alpha='50%'>╰─</span> <span alpha='65%'>[LMB] ncdu ~ · [RMB] btop</span>",
    );

    serde_json::json!({
        "text": ICON_SYSMON,
        "class": pressure.class(),
        "tooltip": tooltip,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_proc_stat_and_computes_delta() {
        let s1 = "cpu  1000 100 300 8000 200 0 0 0 0 0\ncpu0 250 25 75 2000 50 0 0 0 0 0\n";
        let s2 = "cpu  1250 100 400 8600 250 0 0 0 0 0\n";
        let a = parse_proc_stat(s1).unwrap();
        let b = parse_proc_stat(s2).unwrap();
        // total delta = 1000, idle delta = 650 => 35% busy
        assert_eq!(cpu_pct_from_delta(Some(a), b), Some(35));
        assert_eq!(cpu_pct_from_delta(None, b), None);
    }

    #[test]
    fn parses_meminfo_load_and_uptime() {
        let mem = "\
MemTotal:        4096000 kB
MemFree:          500000 kB
MemAvailable:    2048000 kB
SwapTotal:       2097152 kB
SwapFree:        1835008 kB
";
        assert_eq!(
            parse_proc_meminfo(mem),
            Some((2048000, 4096000, 262144, 2097152))
        );
        assert_eq!(
            parse_proc_loadavg("1.42 1.15 0.98 2/412 1991\n"),
            Some(1.42)
        );
        assert_eq!(parse_proc_uptime("15124.87 54000.12\n"), Some(15124));
    }

    #[test]
    fn bar_json_formats_ok_warn_and_critical_states() {
        let snap = SysSnapshot {
            cpu_pct: Some(24),
            cpu_sustained_high: false,
            cores: 8,
            load_1m: Some(1.42),
            temp_c: Some(46),
            mem_used_kb: 1_887_437,
            mem_total_kb: 4_096_000,
            swap_used_kb: 0,
            swap_total_kb: 2_097_152,
            root_disk: Some(DiskUsage {
                mount_label: "/ ~".into(),
                used_bytes: 18_400_000_000,
                total_bytes: 58_000_000_000,
            }),
            home_disk: None,
            same_root_home: true,
            uptime_secs: 4 * 3600 + 12 * 60,
        };
        let raw = bar_json_for(&snap);
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed["class"], "ok");
        assert!(parsed["text"].as_str().unwrap().contains('\u{f4bc}'));
        let tip = parsed["tooltip"].as_str().unwrap();
        assert!(tip.contains("SYS // LIVE"));
        assert!(tip.contains("up 4h 12m"));
        assert!(tip.contains("24%"));
        assert!(tip.contains("46°C"));
        assert!(!tip.contains("SWP"), "idle swap should be omitted: {tip}");
        assert!(tip.contains("[LMB] ncdu ~ · [RMB] btop"));

        // Separate home partition >= 92% triggers warn with word + active swap row
        let mut high = snap.clone();
        high.swap_used_kb = 262_144;
        high.home_disk = Some(DiskUsage {
            mount_label: "~".into(),
            used_bytes: 92,
            total_bytes: 100,
        });
        let high_json: serde_json::Value = serde_json::from_str(&bar_json_for(&high)).unwrap();
        assert_eq!(high_json["class"], "warn");
        let high_tip = high_json["tooltip"].as_str().unwrap();
        assert!(high_tip.contains("SYS // DISK HIGH"));
        assert!(high_tip.contains("SWP"));
        assert!(high_tip.contains("HOM"));

        // >= 96% memory triggers critical
        let mut crit = snap;
        crit.mem_used_kb = 3_950_000;
        let crit_json: serde_json::Value = serde_json::from_str(&bar_json_for(&crit)).unwrap();
        assert_eq!(crit_json["class"], "critical");
        assert!(
            crit_json["tooltip"]
                .as_str()
                .unwrap()
                .contains("SYS // MEM CRIT")
        );
    }
}
