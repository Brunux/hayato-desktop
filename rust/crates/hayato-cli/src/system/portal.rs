//! The network's sign-in page (`hayato menu net --portal`).
//!
//! A hotel, airport or café Wi-Fi answers `nmcli` with `portal` or `limited`
//! until the person accepts its terms. NetworkManager reports that state but
//! not the page; the page is found by asking the same connectivity endpoint
//! NetworkManager uses and reading where it sends us. Nothing is opened until
//! the person picks the row, and only an `http`/`https` URL is accepted.

use std::process::Command;

use super::{have, out};

/// The endpoint NetworkManager's own connectivity check uses on this image.
const PROBE: &str = "http://connectivity-check.ubuntu.com";

/// Does the active connection still need a sign-in page?
pub fn needs_signin() -> bool {
    out(&["nmcli", "-t", "-f", "CONNECTIVITY", "general"])
        .map(|state| needs_signin_from(&state))
        .unwrap_or(false)
}

fn needs_signin_from(state: &str) -> bool {
    matches!(state.trim(), "portal" | "limited")
}

/// The page to open, or `None` when the connection is already online.
pub fn portal_url() -> Option<String> {
    if !needs_signin() {
        return None;
    }
    let output = Command::new("curl")
        .args([
            "-sS",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code}\n%{redirect_url}\n%{url_effective}\n",
            "--max-time",
            "5",
            PROBE,
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    url_from_probe(&String::from_utf8_lossy(&output.stdout))
}

/// `curl -w` prints the status, the redirect and the effective URL, one per
/// line. A portal usually redirects; one that answers in place is the effective
/// URL. `204` is the endpoint saying "online".
fn url_from_probe(text: &str) -> Option<String> {
    let mut lines = text.lines();
    let code = lines.next()?.trim();
    let redirect = lines.next().unwrap_or("").trim();
    let effective = lines.next().unwrap_or("").trim();
    let candidate = if !redirect.is_empty() {
        redirect
    } else if code != "204" {
        effective
    } else {
        ""
    };
    sanitize(candidate)
}

fn sanitize(url: &str) -> Option<String> {
    let url = url.trim();
    (url.starts_with("http://") || url.starts_with("https://")).then(|| url.to_string())
}

/// Open the sign-in page in the browser and return the URL that was opened.
pub fn open() -> Result<String, String> {
    let url = portal_url().ok_or_else(|| "No sign-in page was found".to_string())?;
    // xdg-open reads the default browser (via the shim and hayato-browser);
    // firefox-esr is the image's last resort, not the choice.
    if have("xdg-open") {
        crate::system::spawn_detached_argv("xdg-open", &[&url]);
    } else if have("firefox-esr") {
        crate::system::spawn_detached_argv("firefox-esr", &["--new-tab", &url]);
    } else {
        return Err("No browser is available to open the sign-in page".to_string());
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_portal_or_limited_connection_asks_for_a_page() {
        assert!(needs_signin_from("portal\n"));
        assert!(needs_signin_from("limited\n"));
        assert!(!needs_signin_from("full\n"));
        assert!(!needs_signin_from("none\n"));
        assert!(!needs_signin_from(""));
    }

    #[test]
    fn a_redirect_is_the_page() {
        assert_eq!(
            url_from_probe(
                "302\nhttp://wifi.example/login\nhttp://connectivity-check.ubuntu.com/\n"
            ),
            Some("http://wifi.example/login".to_string())
        );
    }

    #[test]
    fn an_in_place_portal_uses_the_effective_url() {
        assert_eq!(
            url_from_probe("200\n\nhttp://wifi.example/terms\n"),
            Some("http://wifi.example/terms".to_string())
        );
    }

    #[test]
    fn the_online_answer_opens_nothing() {
        assert_eq!(
            url_from_probe("204\n\nhttp://connectivity-check.ubuntu.com/\n"),
            None
        );
    }

    #[test]
    fn only_http_and_https_are_opened() {
        assert_eq!(url_from_probe("302\nfile:///etc/passwd\n\n"), None);
        assert_eq!(url_from_probe("302\n\njavascript:alert(1)\n"), None);
        assert_eq!(
            url_from_probe("302\nhttps://wifi.example/\n\n"),
            Some("https://wifi.example/".to_string())
        );
    }
}
