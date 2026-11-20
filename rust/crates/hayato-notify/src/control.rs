//! The daemon's one-word control socket.
//!
//! Sway keybindings cannot type into a layer-shell card without taking the
//! keyboard away from the application, so they speak to the daemon instead:
//!
//! ```text
//! hayato-notify --control act-newest
//! hayato-notify --control dismiss-newest
//! hayato-notify --control dismiss-all
//! ```
//!
//! The socket lives in `$XDG_RUNTIME_DIR`, mode 0600, and dies with the
//! session. The listener only parses one of the three words; everything else
//! is ignored. A daemon that is not running leaves the command with a message,
//! never a second daemon.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::thread;

use crate::dbus::DbusCommand;
use crate::model::Control;

pub fn socket_path() -> PathBuf {
    std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
        .join("hayato-notify.sock")
}

/// Send one control word to a running daemon. Returns the process exit code.
pub fn send(word: &str) -> i32 {
    let Some(command) = Control::parse(word) else {
        eprintln!("hayato-notify: unknown control word '{word}'");
        eprintln!("try: act-newest, dismiss-newest, dismiss-all");
        return 2;
    };
    let path = socket_path();
    let mut stream = match UnixStream::connect(&path) {
        Ok(stream) => stream,
        Err(error) => {
            eprintln!(
                "hayato-notify: no daemon answers at {}: {error}",
                path.display()
            );
            return 3;
        }
    };
    let line = match command {
        Control::ActNewest => "act-newest",
        Control::DismissNewest => "dismiss-newest",
        Control::DismissAll => "dismiss-all",
    };
    if let Err(error) = writeln!(stream, "{line}") {
        eprintln!("hayato-notify: could not send '{line}': {error}");
        return 3;
    }
    0
}

/// Bind the socket and start the listener thread. The returned guard removes
/// the socket file when the daemon exits.
pub fn listen(tx: Sender<DbusCommand>, wake_fd: i32) -> std::io::Result<SocketGuard> {
    let path = socket_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if path.exists() {
        // A live socket belongs to the daemon that already owns the session.
        // A stale one is a file left by a crash, and can be replaced.
        if UnixStream::connect(&path).is_ok() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                "another hayato-notify is already listening",
            ));
        }
        let _ = std::fs::remove_file(&path);
    }
    let listener = UnixListener::bind(&path)?;
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    let guard = SocketGuard { path: path.clone() };
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut line = String::new();
            let mut reader = BufReader::new(stream);
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            let Some(command) = Control::parse(&line) else {
                continue;
            };
            if tx.send(DbusCommand::Control(command)).is_ok() {
                wake(wake_fd);
            }
        }
    });
    Ok(guard)
}

fn wake(wake_fd: i32) {
    if wake_fd >= 0 {
        let value: u64 = 1;
        unsafe {
            libc::write(
                wake_fd,
                &value as *const _ as *const libc::c_void,
                std::mem::size_of::<u64>(),
            );
        }
    }
}

pub struct SocketGuard {
    path: PathBuf,
}

impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_words_round_trip() {
        for word in ["act-newest", "dismiss-newest", "dismiss-all"] {
            assert!(Control::parse(word).is_some(), "{word}");
        }
        assert!(Control::parse("open").is_none());
    }
}
