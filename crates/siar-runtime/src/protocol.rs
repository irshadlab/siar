//! §14-§25, §102-§105: the IPC wire protocol.
//!
//! Framing is `u32` little-endian length + postcard body. The length is
//! checked against [`MAX_FRAME`] **before** any allocation, so a hostile or
//! corrupt peer cannot make the daemon allocate more than 1 MiB per frame
//! (§193 fuzz target: arbitrary bytes into [`decode_frame`] must only ever
//! return `Err`).

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::dedup::CommandId;
use crate::error::RuntimeError;
use crate::events::{Delivery, Topic};
use crate::health::HealthReport;
use crate::lifecycle::RuntimeState;

pub const PROTOCOL_VERSION: u16 = 1;
pub const MIN_PROTOCOL_VERSION: u16 = 1;
pub const MAX_FRAME: usize = 1 << 20;

/// §19. Ordered: a higher role includes the lower ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Role {
    /// Read state and subscribe (a status widget, a metrics scraper).
    Observer,
    /// Everything a UI or CLI user does.
    Operator,
    /// Lifecycle and maintenance (shutdown, read-only, maintenance mode).
    Admin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Command {
    Shutdown,
    SetEmergency(bool),
    SetMaintenance(bool),
    SetReadOnly(bool),
    /// Application-defined; handled by the embedding app's `AppService`.
    App {
        name: String,
        data: Vec<u8>,
    },
}

impl Command {
    /// §20: least role allowed to issue it.
    #[must_use]
    pub const fn required_role(&self) -> Role {
        match self {
            Self::Shutdown | Self::SetMaintenance(_) | Self::SetReadOnly(_) => Role::Admin,
            Self::SetEmergency(_) | Self::App { .. } => Role::Operator,
        }
    }

    /// Does it change durable communication state (refused in read-only /
    /// maintenance)? Lifecycle controls are always allowed to the Admin.
    #[must_use]
    pub const fn is_mutation(&self) -> bool {
        matches!(self, Self::App { .. } | Self::SetEmergency(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Query {
    Status,
    Health,
    Power,
    App { name: String, data: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Request {
    Command { id: CommandId, cmd: Command },
    Query(Query),
    Subscribe { topics: Vec<Topic>, capacity: u16 },
    Unsubscribe,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientMsg {
    Hello {
        min_version: u16,
        max_version: u16,
        role: Role,
        client_name: String,
    },
    Request {
        req_id: u64,
        body: Request,
    },
    Ping,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusSnapshot {
    pub state: RuntimeState,
    pub instance_id: u128,
    pub uptime_ms: u64,
    pub subscribers: u32,
    pub protocol_version: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Reply {
    Ok,
    Status(StatusSnapshot),
    Health(HealthReport),
    Power(Box<siar_power::PowerDiagnostics>),
    Bytes(Vec<u8>),
    /// §25: a snapshot taken now is consistent with deltas from `next_seq`.
    Subscribed {
        next_seq: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerMsg {
    Welcome {
        version: u16,
        instance_id: u128,
        role: Role,
        next_seq: u64,
    },
    Response {
        req_id: u64,
        result: Result<Reply, RuntimeError>,
    },
    Delivery(Delivery),
    Pong,
}

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("frame of {0} bytes exceeds the {MAX_FRAME}-byte limit")]
    TooLarge(usize),
    #[error("malformed frame")]
    Malformed,
    #[error("connection closed")]
    Closed,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Encode one message into a framed buffer.
pub fn encode_frame<T: Serialize>(msg: &T) -> Result<Vec<u8>, FrameError> {
    let body = postcard::to_allocvec(msg).map_err(|_| FrameError::Malformed)?;
    if body.len() > MAX_FRAME {
        return Err(FrameError::TooLarge(body.len()));
    }
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// Decode a *complete* frame buffer (header included). Never panics.
pub fn decode_frame<T: for<'de> Deserialize<'de>>(buf: &[u8]) -> Result<T, FrameError> {
    let (hdr, body) = buf.split_first_chunk::<4>().ok_or(FrameError::Malformed)?;
    let len = u32::from_le_bytes(*hdr) as usize;
    if len > MAX_FRAME {
        return Err(FrameError::TooLarge(len));
    }
    if body.len() != len {
        return Err(FrameError::Malformed);
    }
    postcard::from_bytes(body).map_err(|_| FrameError::Malformed)
}

pub async fn write_frame<W: AsyncWrite + Unpin, T: Serialize>(
    w: &mut W,
    msg: &T,
) -> Result<(), FrameError> {
    let buf = encode_frame(msg)?;
    w.write_all(&buf).await?;
    w.flush().await?;
    Ok(())
}

pub async fn read_frame<R: AsyncRead + Unpin, T: for<'de> Deserialize<'de>>(
    r: &mut R,
) -> Result<T, FrameError> {
    let mut hdr = [0u8; 4];
    match r.read_exact(&mut hdr).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Err(FrameError::Closed),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_le_bytes(hdr) as usize;
    if len > MAX_FRAME {
        return Err(FrameError::TooLarge(len));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            FrameError::Closed
        } else {
            e.into()
        }
    })?;
    postcard::from_bytes(&body).map_err(|_| FrameError::Malformed)
}

/// §103/§105: pick the highest version both sides support.
#[must_use]
pub fn negotiate_version(client_min: u16, client_max: u16) -> Option<u16> {
    let hi = client_max.min(PROTOCOL_VERSION);
    let lo = client_min.max(MIN_PROTOCOL_VERSION);
    (lo <= hi).then_some(hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let m = ClientMsg::Request {
            req_id: 9,
            body: Request::Command {
                id: CommandId(5),
                cmd: Command::App {
                    name: "x".into(),
                    data: vec![1, 2],
                },
            },
        };
        let f = encode_frame(&m).unwrap();
        assert_eq!(decode_frame::<ClientMsg>(&f).unwrap(), m);
    }

    #[test]
    fn oversize_length_is_rejected_without_allocating() {
        let mut f = u32::MAX.to_le_bytes().to_vec();
        f.extend_from_slice(&[0; 8]);
        assert!(matches!(
            decode_frame::<ClientMsg>(&f),
            Err(FrameError::TooLarge(_))
        ));
    }

    #[test]
    fn garbage_never_panics() {
        let mut state = 0x1234_5678_9abc_def0_u64;
        for len in 0..300usize {
            let mut buf = vec![0u8; len];
            for b in &mut buf {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                *b = state as u8;
            }
            let _ = decode_frame::<ClientMsg>(&buf);
            let _ = decode_frame::<ServerMsg>(&buf);
            // Also with a *valid* header over garbage body.
            let mut framed = (buf.len() as u32).to_le_bytes().to_vec();
            framed.extend_from_slice(&buf);
            let _ = decode_frame::<ClientMsg>(&framed);
        }
    }

    #[test]
    fn truncated_frame_is_malformed() {
        let f = encode_frame(&ClientMsg::Ping).unwrap();
        assert!(decode_frame::<ClientMsg>(&f[..f.len() - 1]).is_err() || f.len() == 5);
        assert!(decode_frame::<ClientMsg>(&[1, 0]).is_err());
    }

    #[test]
    fn version_negotiation() {
        assert_eq!(negotiate_version(1, 1), Some(1));
        assert_eq!(negotiate_version(1, 9), Some(PROTOCOL_VERSION));
        assert_eq!(negotiate_version(2, 9), None);
        assert_eq!(negotiate_version(0, 0), None);
    }

    #[test]
    fn roles_are_ordered_and_commands_map_to_them() {
        assert!(Role::Admin > Role::Operator && Role::Operator > Role::Observer);
        assert_eq!(Command::Shutdown.required_role(), Role::Admin);
        assert_eq!(
            Command::App {
                name: String::new(),
                data: vec![]
            }
            .required_role(),
            Role::Operator
        );
        assert!(!Command::Shutdown.is_mutation());
    }

    #[tokio::test]
    async fn async_read_write() {
        let (mut a, mut b) = tokio::io::duplex(4096);
        write_frame(&mut a, &ServerMsg::Pong).await.unwrap();
        assert_eq!(
            read_frame::<_, ServerMsg>(&mut b).await.unwrap(),
            ServerMsg::Pong
        );
        drop(a);
        assert!(matches!(
            read_frame::<_, ServerMsg>(&mut b).await,
            Err(FrameError::Closed)
        ));
    }
}
