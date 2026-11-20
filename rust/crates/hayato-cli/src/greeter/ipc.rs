//! Typed `greetd` IPC wire client (`GREETD_SOCK`).
//!
//! Speaks `greetd`'s length-prefixed JSON protocol (`u32` native-endian byte
//! count followed by a UTF-8 JSON object) directly over a `UnixStream`, with a
//! dry-run [`PreviewBackend`] for desktop previews (`hayato greeter --preview`)
//! and `UnixStream::pair()` unit tests.

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Reject abnormally large IPC payloads before allocating a buffer.
const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// Socket timeout so a stalled daemon cannot wedge the background worker.
const IO_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMessageType {
    Visible,
    Secret,
    Info,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorType {
    AuthError,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    CreateSession { username: String },
    PostAuthMessageResponse { response: Option<String> },
    StartSession { cmd: Vec<String>, env: Vec<String> },
    CancelSession,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Success,
    Error {
        error_type: ErrorType,
        description: String,
    },
    AuthMessage {
        auth_message_type: AuthMessageType,
        auth_message: String,
    },
}

/// Write one length-prefixed JSON request frame.
pub fn write_frame<W: Write>(writer: &mut W, req: &Request) -> io::Result<()> {
    let json = serde_json::to_vec(req)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("encode error: {e}")))?;
    if json.len() > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "greetd IPC request exceeds max frame size",
        ));
    }
    let len = (json.len() as u32).to_ne_bytes();
    writer.write_all(&len)?;
    writer.write_all(&json)?;
    writer.flush()
}

/// Read one length-prefixed JSON response frame.
pub fn read_frame<R: Read>(reader: &mut R) -> io::Result<Response> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf)?;
    let len = u32::from_ne_bytes(len_buf) as usize;
    if len == 0 || len > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid greetd IPC frame length: {len}"),
        ));
    }
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload)?;
    serde_json::from_slice(&payload).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid greetd JSON response: {e}"),
        )
    })
}

/// Exchange one request/response pair over an arbitrary stream.
pub fn transact<S: Read + Write>(stream: &mut S, req: &Request) -> io::Result<Response> {
    write_frame(stream, req)?;
    read_frame(stream)
}

/// Outcome of submitting credentials or a follow-up PAM prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthOutcome {
    /// PAM authenticated and `start_session` succeeded (`greetd` is launching the user session).
    Started { preview_note: Option<String> },
    /// PAM requires another interactive answer (e.g. OTP/2FA) on the open socket.
    Challenge {
        prompt_type: AuthMessageType,
        prompt: String,
        notice: Option<String>,
    },
    /// PAM rejected credentials or the session failed; any active PAM session was cancelled.
    Failed(String),
}

/// Launch instructions passed into `start_session`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionLaunch {
    pub username: String,
    pub cmd: Vec<String>,
    pub env: Vec<String>,
    pub session_name: String,
}

/// Stateful client holding at most one live `UnixStream` across multi-step PAM challenges.
pub struct SocketAuthSession {
    sock_path: PathBuf,
    stream: Option<UnixStream>,
}

impl SocketAuthSession {
    pub fn new(sock_path: PathBuf) -> Self {
        Self {
            sock_path,
            stream: None,
        }
    }

    pub fn cancel(&mut self) {
        if let Some(mut stream) = self.stream.take() {
            let _ = transact(&mut stream, &Request::CancelSession);
        }
    }

    /// Start a new PAM session for `launch.username`, automatically answering the
    /// initial `Secret`/`Visible` prompt with `secret` if PAM asks immediately.
    pub fn begin(&mut self, launch: &SessionLaunch, secret: &str) -> AuthOutcome {
        self.cancel();
        let mut stream = match UnixStream::connect(&self.sock_path) {
            Ok(s) => s,
            Err(e) => {
                return AuthOutcome::Failed(format!(
                    "cannot reach greetd socket {}: {e}",
                    self.sock_path.display()
                ));
            }
        };
        let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
        let _ = stream.set_write_timeout(Some(IO_TIMEOUT));

        let resp = match transact(
            &mut stream,
            &Request::CreateSession {
                username: launch.username.clone(),
            },
        ) {
            Ok(r) => r,
            Err(e) => return AuthOutcome::Failed(format!("greetd handshake failed: {e}")),
        };

        self.drive(stream, launch, resp, Some(secret))
    }

    /// Answer a pending `Challenge` prompt on the already-open PAM session.
    pub fn reply(&mut self, launch: &SessionLaunch, answer: &str) -> AuthOutcome {
        let Some(mut stream) = self.stream.take() else {
            return self.begin(launch, answer);
        };
        let resp = match transact(
            &mut stream,
            &Request::PostAuthMessageResponse {
                response: Some(answer.to_string()),
            },
        ) {
            Ok(r) => r,
            Err(e) => {
                return AuthOutcome::Failed(format!("greetd auth reply failed: {e}"));
            }
        };
        self.drive(stream, launch, resp, None)
    }

    fn drive(
        &mut self,
        mut stream: UnixStream,
        launch: &SessionLaunch,
        mut current: Response,
        mut initial_secret: Option<&str>,
    ) -> AuthOutcome {
        let mut notice: Option<String> = None;

        for _step in 0..16 {
            match current {
                Response::Success => {
                    let start_resp = transact(
                        &mut stream,
                        &Request::StartSession {
                            cmd: launch.cmd.clone(),
                            env: launch.env.clone(),
                        },
                    );
                    match start_resp {
                        Ok(Response::Success) => {
                            return AuthOutcome::Started { preview_note: None };
                        }
                        Ok(Response::Error { description, .. }) => {
                            let _ = transact(&mut stream, &Request::CancelSession);
                            return AuthOutcome::Failed(format!(
                                "session start refused: {description}"
                            ));
                        }
                        Ok(Response::AuthMessage { auth_message, .. }) => {
                            let _ = transact(&mut stream, &Request::CancelSession);
                            return AuthOutcome::Failed(format!(
                                "unexpected auth prompt after start: {auth_message}"
                            ));
                        }
                        Err(e) => {
                            return AuthOutcome::Failed(format!("failed to start session: {e}"));
                        }
                    }
                }
                Response::Error {
                    error_type,
                    description,
                } => {
                    let _ = transact(&mut stream, &Request::CancelSession);
                    let msg = match (error_type, description.trim()) {
                        (ErrorType::AuthError, "") => {
                            "Authentication failed — check username and password".to_string()
                        }
                        (_, "") => "Session error from greetd".to_string(),
                        (_, text) => text.to_string(),
                    };
                    return AuthOutcome::Failed(msg);
                }
                Response::AuthMessage {
                    auth_message_type,
                    auth_message,
                } => match auth_message_type {
                    AuthMessageType::Info | AuthMessageType::Error => {
                        notice = Some(auth_message);
                        match transact(
                            &mut stream,
                            &Request::PostAuthMessageResponse { response: None },
                        ) {
                            Ok(next) => current = next,
                            Err(e) => {
                                let _ = transact(&mut stream, &Request::CancelSession);
                                return AuthOutcome::Failed(format!("greetd PAM ack failed: {e}"));
                            }
                        }
                    }
                    AuthMessageType::Secret | AuthMessageType::Visible => {
                        if let Some(secret) = initial_secret.take() {
                            match transact(
                                &mut stream,
                                &Request::PostAuthMessageResponse {
                                    response: Some(secret.to_string()),
                                },
                            ) {
                                Ok(next) => current = next,
                                Err(e) => {
                                    let _ = transact(&mut stream, &Request::CancelSession);
                                    return AuthOutcome::Failed(format!(
                                        "greetd password send failed: {e}"
                                    ));
                                }
                            }
                        } else {
                            self.stream = Some(stream);
                            return AuthOutcome::Challenge {
                                prompt_type: auth_message_type,
                                prompt: auth_message,
                                notice,
                            };
                        }
                    }
                },
            }
        }

        let _ = transact(&mut stream, &Request::CancelSession);
        AuthOutcome::Failed("PAM conversation exceeded step limit".to_string())
    }
}

impl Drop for SocketAuthSession {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Abstract backend so the TUI works seamlessly both against real `GREETD_SOCK`
/// and in `--preview` / test mode.
pub enum Backend {
    Socket(SocketAuthSession),
    Preview,
}

impl Backend {
    pub fn from_env_or_preview(preview: bool) -> Result<Self, String> {
        if preview {
            return Ok(Self::Preview);
        }
        match std::env::var("GREETD_SOCK") {
            Ok(sock) if !sock.trim().is_empty() => {
                Ok(Self::Socket(SocketAuthSession::new(PathBuf::from(sock))))
            }
            _ => Err(
                "GREETD_SOCK is not set (use `hayato greeter --preview` to test from a session)"
                    .into(),
            ),
        }
    }

    pub fn begin(&mut self, launch: &SessionLaunch, secret: &str) -> AuthOutcome {
        match self {
            Self::Socket(client) => client.begin(launch, secret),
            Self::Preview => preview_check(launch, secret),
        }
    }

    pub fn reply(&mut self, launch: &SessionLaunch, answer: &str) -> AuthOutcome {
        match self {
            Self::Socket(client) => client.reply(launch, answer),
            Self::Preview => preview_check(launch, answer),
        }
    }

    pub fn cancel(&mut self) {
        if let Self::Socket(client) = self {
            client.cancel();
        }
    }
}

fn preview_check(launch: &SessionLaunch, secret: &str) -> AuthOutcome {
    if launch.username.trim().is_empty() {
        return AuthOutcome::Failed("Enter a username before signing in".into());
    }
    if secret == "wrong" {
        return AuthOutcome::Failed("Authentication failed (preview simulated rejection)".into());
    }
    AuthOutcome::Started {
        preview_note: Some(format!(
            "Preview: would start '{}' ({}) as '{}'",
            launch.session_name,
            launch.cmd.join(" "),
            launch.username
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    fn read_req<R: Read>(r: &mut R) -> Request {
        let mut len_buf = [0u8; 4];
        r.read_exact(&mut len_buf).unwrap();
        let len = u32::from_ne_bytes(len_buf) as usize;
        let mut payload = vec![0u8; len];
        r.read_exact(&mut payload).unwrap();
        serde_json::from_slice(&payload).unwrap()
    }

    fn write_resp<W: Write>(w: &mut W, resp: &Response) {
        let json = serde_json::to_vec(resp).unwrap();
        let len = (json.len() as u32).to_ne_bytes();
        w.write_all(&len).unwrap();
        w.write_all(&json).unwrap();
        w.flush().unwrap();
    }

    #[test]
    fn frame_round_trip_and_oversized_guard() {
        let req = Request::CreateSession {
            username: "brunux".into(),
        };
        let mut buf = Vec::new();
        write_frame(&mut buf, &req).unwrap();
        assert!(buf.len() > 4);

        let mut oversize = (MAX_FRAME_BYTES as u32 + 1).to_ne_bytes().to_vec();
        oversize.extend_from_slice(b"{}");
        let err = read_frame(&mut io::Cursor::new(oversize)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn full_socket_login_handshake_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("greetd.sock");
        let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();

        let server = thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            assert_eq!(
                read_req(&mut conn),
                Request::CreateSession {
                    username: "brunux".into()
                }
            );
            write_resp(
                &mut conn,
                &Response::AuthMessage {
                    auth_message_type: AuthMessageType::Secret,
                    auth_message: "Password:".into(),
                },
            );
            assert_eq!(
                read_req(&mut conn),
                Request::PostAuthMessageResponse {
                    response: Some("debian".into())
                }
            );
            write_resp(&mut conn, &Response::Success);
            assert_eq!(
                read_req(&mut conn),
                Request::StartSession {
                    cmd: vec!["sway".into()],
                    env: vec!["XDG_SESSION_TYPE=wayland".into()],
                }
            );
            write_resp(&mut conn, &Response::Success);
        });

        let mut client = SocketAuthSession::new(sock);
        let launch = SessionLaunch {
            username: "brunux".into(),
            cmd: vec!["sway".into()],
            env: vec!["XDG_SESSION_TYPE=wayland".into()],
            session_name: "Sway".into(),
        };
        let outcome = client.begin(&launch, "debian");
        assert_eq!(outcome, AuthOutcome::Started { preview_note: None });
        server.join().unwrap();
    }

    #[test]
    fn auth_error_cancels_session_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("greetd.sock");
        let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();

        let server = thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let _ = read_req(&mut conn);
            write_resp(
                &mut conn,
                &Response::AuthMessage {
                    auth_message_type: AuthMessageType::Secret,
                    auth_message: "Password:".into(),
                },
            );
            let _ = read_req(&mut conn);
            write_resp(
                &mut conn,
                &Response::Error {
                    error_type: ErrorType::AuthError,
                    description: String::new(),
                },
            );
            assert_eq!(read_req(&mut conn), Request::CancelSession);
        });

        let mut client = SocketAuthSession::new(sock);
        let launch = SessionLaunch {
            username: "brunux".into(),
            cmd: vec!["sway".into()],
            env: vec![],
            session_name: "Sway".into(),
        };
        let outcome = client.begin(&launch, "badpass");
        match outcome {
            AuthOutcome::Failed(msg) => assert!(msg.contains("Authentication failed"), "{msg}"),
            other => panic!("expected Failed, got {other:?}"),
        }
        server.join().unwrap();
    }

    #[test]
    fn multi_step_pam_info_and_visible_otp_challenge_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("greetd.sock");
        let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();

        let server = thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let _ = read_req(&mut conn);
            write_resp(
                &mut conn,
                &Response::AuthMessage {
                    auth_message_type: AuthMessageType::Info,
                    auth_message: "Two-factor enabled".into(),
                },
            );
            assert_eq!(
                read_req(&mut conn),
                Request::PostAuthMessageResponse { response: None }
            );
            write_resp(
                &mut conn,
                &Response::AuthMessage {
                    auth_message_type: AuthMessageType::Secret,
                    auth_message: "Password:".into(),
                },
            );
            assert_eq!(
                read_req(&mut conn),
                Request::PostAuthMessageResponse {
                    response: Some("pw".into())
                }
            );
            write_resp(
                &mut conn,
                &Response::AuthMessage {
                    auth_message_type: AuthMessageType::Visible,
                    auth_message: "OTP code:".into(),
                },
            );
            assert_eq!(
                read_req(&mut conn),
                Request::PostAuthMessageResponse {
                    response: Some("123456".into())
                }
            );
            write_resp(&mut conn, &Response::Success);
            let _ = read_req(&mut conn);
            write_resp(&mut conn, &Response::Success);
        });

        let mut client = SocketAuthSession::new(sock);
        let launch = SessionLaunch {
            username: "brunux".into(),
            cmd: vec!["sway".into()],
            env: vec![],
            session_name: "Sway".into(),
        };
        let step1 = client.begin(&launch, "pw");
        assert_eq!(
            step1,
            AuthOutcome::Challenge {
                prompt_type: AuthMessageType::Visible,
                prompt: "OTP code:".into(),
                notice: Some("Two-factor enabled".into()),
            }
        );
        let step2 = client.reply(&launch, "123456");
        assert_eq!(step2, AuthOutcome::Started { preview_note: None });
        server.join().unwrap();
    }
}
