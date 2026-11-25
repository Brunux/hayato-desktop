//! Dynamic V4L2 webcam discovery under `/sys/class/video4linux`.
//!
//! Never assumes `/dev/video0`: on MediaTek SoCs (`mt8192` / `mt8183`),
//! hardware codec nodes (`mtk-vcodec-enc`, `mtk-vcodec-dec`) can claim
//! `/dev/video0`, pushing the USB UVC webcam to `/dev/video1` (with `video2`
//! as its `index == 1` metadata node).

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebcamInfo {
    pub device: String,
    pub name: String,
}

pub fn find_webcam_in(sys_dir: &Path) -> Option<WebcamInfo> {
    let entries = std::fs::read_dir(sys_dir).ok()?;
    let mut nodes: Vec<(u32, String, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let num: u32 = name.strip_prefix("video")?.parse().ok()?;
            Some((num, name, entry.path()))
        })
        .collect();
    nodes.sort_by_key(|(num, _, _)| *num);

    let mut fallback: Option<WebcamInfo> = None;
    for (_, node_name, path) in nodes {
        let index = std::fs::read_to_string(path.join("index"))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "0".to_string());
        if index != "0" {
            continue;
        }
        let dev_label = std::fs::read_to_string(path.join("name"))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| node_name.clone());
        let driver = std::fs::canonicalize(path.join("device/driver"))
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_default();
        if driver == "uvcvideo" {
            return Some(WebcamInfo {
                device: format!("/dev/{node_name}"),
                name: dev_label,
            });
        }
        let low_name = dev_label.to_ascii_lowercase();
        let low_drv = driver.to_ascii_lowercase();
        let is_codec = [
            "vcodec", "codec", "m2m", "encoder", "decoder", "isp", "meta",
        ]
        .iter()
        .any(|needle| low_name.contains(needle) || low_drv.contains(needle));
        if !is_codec && fallback.is_none() {
            fallback = Some(WebcamInfo {
                device: format!("/dev/{node_name}"),
                name: dev_label,
            });
        }
    }
    fallback
}

pub fn find_webcam() -> Option<WebcamInfo> {
    find_webcam_in(Path::new("/sys/class/video4linux"))
}
