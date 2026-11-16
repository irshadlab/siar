//! Talking to the daemon: connect (optionally starting it), issue app
//! requests, and turn failures into actionable messages.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, Result};
use siar_runtime::ipc::{default_socket_path, ClientError, IpcClient};
use siar_runtime::{Command, CommandId, Query, Reply, Request, Role, RuntimeError};

const CLIENT_NAME: &str = "siar-cli";
/// How long to wait for a freshly started daemon to open its socket.
const SPAWN_TIMEOUT: Duration = Duration::from_secs(20);
/// The daemon answers `NotReady` until its core finished starting.
const READY_TIMEOUT: Duration = Duration::from_secs(30);

pub struct Daemon {
    client: IpcClient,
}

/// Where the daemon is expected, for messages and for connecting.
#[must_use]
pub fn socket_or_default(explicit: Option<PathBuf>) -> PathBuf {
    explicit.unwrap_or_else(|| default_socket_path("daemon"))
}

/// The one error a user hits first: no daemon.
#[must_use]
pub fn not_running_message(socket: &Path) -> String {
    format!(
        "siar-daemon is not running (nothing is listening on {}).\n\
         Start it with `siar-daemon run`, or add `--spawn` to start it for you.",
        socket.display()
    )
}

impl Daemon {
    pub async fn connect(socket: &Path, spawn: bool) -> Result<Self> {
        let client = if spawn {
            let s = socket.to_path_buf();
            siar_runtime::ensure::ensure_daemon(
                socket,
                Role::Operator,
                CLIENT_NAME,
                move || async move { siar_runtime::ensure::spawn_daemon_beside_exe(&s) },
                SPAWN_TIMEOUT,
            )
            .await
            .map_err(|e| anyhow!("could not start or reach siar-daemon: {e}"))?
        } else {
            IpcClient::connect_local(socket, Role::Operator, CLIENT_NAME)
                .await
                .map_err(|e| match e {
                    ClientError::Io(_) => anyhow!(not_running_message(socket)),
                    other => anyhow!("siar-daemon refused the connection: {other}"),
                })?
        };
        Ok(Self { client })
    }

    /// Send `req`, retrying while the daemon reports it is still starting.
    /// The same request (and so the same `CommandId`) is resent, so a retry
    /// can never run a mutation twice.
    async fn call(&mut self, req: Request) -> Result<Reply> {
        let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
        loop {
            match self.client.request(req.clone()).await {
                Ok(v) => return Ok(v),
                Err(ClientError::Runtime(RuntimeError::NotReady))
                    if tokio::time::Instant::now() < deadline =>
                {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
                Err(e) => return Err(explain(e)),
            }
        }
    }

    pub async fn query(&mut self, name: &str, data: &str) -> Result<String> {
        let reply = self
            .call(Request::Query(Query::App {
                name: name.to_owned(),
                data: data.as_bytes().to_vec(),
            }))
            .await?;
        Ok(text_of(reply))
    }

    /// One mutation = one fresh `CommandId`.
    pub async fn command(&mut self, name: &str, data: &str) -> Result<String> {
        let reply = self
            .call(Request::Command {
                id: CommandId(uuid::Uuid::new_v4().as_u128()),
                cmd: Command::App {
                    name: name.to_owned(),
                    data: data.as_bytes().to_vec(),
                },
            })
            .await?;
        Ok(text_of(reply))
    }

    pub async fn status(&mut self) -> Result<String> {
        Ok(match self.call(Request::Query(Query::Status)).await? {
            Reply::Status(s) => format!(
                "state: {:?}\nuptime: {} s\nbuild: {}",
                s.state,
                s.uptime_ms / 1000,
                s.build
            ),
            other => text_of(other),
        })
    }
}

fn text_of(reply: Reply) -> String {
    match reply {
        Reply::Bytes(b) => String::from_utf8_lossy(&b).into_owned(),
        _ => String::new(),
    }
}

fn explain(e: ClientError) -> anyhow::Error {
    match e {
        ClientError::Runtime(RuntimeError::NotReady) => {
            anyhow!("siar-daemon is still starting up; try again in a moment")
        }
        ClientError::Runtime(RuntimeError::Busy) => {
            anyhow!("siar-daemon is busy; try again in a moment")
        }
        ClientError::Runtime(RuntimeError::InvalidCommand(why)) => anyhow!("{why}"),
        ClientError::Io(_) | ClientError::Frame(_) => {
            anyhow!("lost the connection to siar-daemon (did it stop?)")
        }
        other => anyhow!("{other}"),
    }
}

/// What a conversation target means for `send`: an id, or a ticket to pair.
#[derive(Debug, PartialEq, Eq)]
pub enum Target<'a> {
    Conversation(&'a str),
    Ticket(&'a str),
}

#[must_use]
pub fn classify_target(t: &str) -> Target<'_> {
    if uuid::Uuid::parse_str(t.trim()).is_ok() {
        Target::Conversation(t.trim())
    } else {
        Target::Ticket(t.trim())
    }
}

/// Lines in `new` that were not already at the end of `old`.
///
/// `messages` returns the newest window of a conversation, so successive
/// snapshots overlap: find the longest suffix of `old` that is a prefix of
/// `new` and report only what follows. Identical consecutive messages are
/// handled because the overlap is positional, not set-based.
#[must_use]
pub fn fresh_lines<'a>(old: &[String], new: &'a [String]) -> &'a [String] {
    let max = old.len().min(new.len());
    for k in (0..=max).rev() {
        if old[old.len() - k..] == new[..k] {
            return &new[k..];
        }
    }
    new
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn the_no_daemon_message_says_what_to_do() {
        let m = not_running_message(Path::new("/run/x/daemon.sock"));
        assert!(m.contains("/run/x/daemon.sock"));
        assert!(m.contains("siar-daemon run"));
        assert!(m.contains("--spawn"));
    }

    #[test]
    fn targets_are_told_apart() {
        let id = "6f9619ff-8b86-d011-b42d-00c04fc964ff";
        assert_eq!(classify_target(id), Target::Conversation(id));
        assert_eq!(
            classify_target(" ticketbase32 "),
            Target::Ticket("ticketbase32")
        );
    }

    #[test]
    fn only_new_lines_are_reported() {
        assert_eq!(fresh_lines(&v(&[]), &v(&["a", "b"])), &v(&["a", "b"])[..]);
        assert_eq!(
            fresh_lines(&v(&["a", "b"]), &v(&["a", "b"])),
            &[] as &[String]
        );
        assert_eq!(
            fresh_lines(&v(&["a", "b"]), &v(&["a", "b", "c"])),
            &v(&["c"])[..]
        );
        // The window slid: the oldest line fell off, one new one arrived.
        assert_eq!(
            fresh_lines(&v(&["a", "b", "c"]), &v(&["b", "c", "d"])),
            &v(&["d"])[..]
        );
        // Repeated identical messages still count.
        assert_eq!(
            fresh_lines(&v(&["x", "x"]), &v(&["x", "x", "x"])),
            &v(&["x"])[..]
        );
        // No overlap at all (a long gap): everything is new.
        assert_eq!(
            fresh_lines(&v(&["a"]), &v(&["z", "y"])),
            &v(&["z", "y"])[..]
        );
    }
}
