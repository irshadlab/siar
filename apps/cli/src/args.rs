//! Command-line grammar for `siar` (clap derive). The CLI is a thin client:
//! every command is a request to a running `siar-daemon`.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// Command-line client for `siar-daemon`.
///
/// The daemon owns the database, keys and network identity; this tool never
/// opens them, so it cannot collide with a running window or daemon.
#[derive(Debug, Parser)]
#[command(name = "siar", version, about)]
pub struct Args {
    /// Daemon control socket (default: the per-user runtime directory).
    #[arg(long, global = true, value_name = "PATH")]
    pub socket: Option<PathBuf>,
    /// Start `siar-daemon` (found beside this program) if it is not running.
    #[arg(long, global = true)]
    pub spawn: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, PartialEq, Eq, Subcommand)]
pub enum Command {
    /// Daemon and messaging summary.
    Status,
    /// Print this device's pairing ticket (give it to a peer).
    Ticket,
    /// Print this device's id and account id.
    Identity,
    /// Print `<device-id> <account-id> <key package>` for a group admin.
    KeyPackage,
    /// List conversations.
    Conversations,
    /// List groups.
    Groups,
    /// Show the newest messages of a conversation.
    Messages { conversation: String },
    /// Pair with a peer's ticket; prints the conversation id.
    Pair { ticket: String },
    /// Send a text message. `target` is a conversation id, or a peer ticket
    /// (which is paired first).
    Send { target: String, text: String },
    /// Send a file to a direct conversation (or to a ticket, paired first).
    SendFile { target: String, path: PathBuf },
    /// Leave a message for a peer at a relay (anonymous token mailbox). The
    /// peer collects it with `check-mailbox-anon`.
    SendAnon {
        target: String,
        relay_ticket: String,
        text: String,
    },
    /// Collect messages a peer left for you at a relay; prints what arrives.
    CheckMailboxAnon {
        target: String,
        relay_ticket: String,
    },
    /// List this account's devices.
    Devices,
    /// Open a one-time offer to add another device; prints the code and ticket
    /// to give to it. Only the device that created the account can do this.
    LinkOffer,
    /// Devices waiting for you to approve them.
    LinkPending,
    /// Approve a waiting device (check its key fingerprint first).
    LinkApprove { device_id: String },
    /// Decline a waiting device.
    LinkReject { device_id: String },
    /// Join an account from a fresh device, with an offer's ticket and code.
    LinkJoin { ticket: String, code: String },
    /// Follow a conversation and print new messages as they arrive.
    Listen { conversation: String },
    /// Create a new MLS group.
    GroupCreate,
    /// Add a member (get the last three values from their `key-package`).
    GroupAdd {
        conversation: String,
        peer_ticket: String,
        peer_device_id: String,
        peer_account_id: String,
        base64_key_package: String,
    },
    /// Send a message to a group.
    GroupSend { conversation: String, text: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(a: &[&str]) -> Result<Args, clap::Error> {
        Args::try_parse_from(std::iter::once("siar").chain(a.iter().copied()))
    }

    #[test]
    fn global_flags_may_come_before_or_after_the_command() {
        let a = parse(&["--spawn", "--socket", "/s", "status"]).unwrap();
        assert!(a.spawn);
        assert_eq!(a.socket, Some(PathBuf::from("/s")));
        let b = parse(&["status", "--spawn"]).unwrap();
        assert!(b.spawn);
        assert_eq!(b.command, Command::Status);
    }

    #[test]
    fn positional_commands_parse_in_order() {
        assert_eq!(
            parse(&["send", "t", "hello world"]).unwrap().command,
            Command::Send {
                target: "t".into(),
                text: "hello world".into()
            }
        );
        assert_eq!(
            parse(&["group-add", "c", "t", "d", "a", "kp"])
                .unwrap()
                .command,
            Command::GroupAdd {
                conversation: "c".into(),
                peer_ticket: "t".into(),
                peer_device_id: "d".into(),
                peer_account_id: "a".into(),
                base64_key_package: "kp".into(),
            }
        );
    }

    #[test]
    fn send_file_takes_a_target_and_a_path() {
        assert_eq!(
            parse(&["send-file", "t", "/tmp/a b.png"]).unwrap().command,
            Command::SendFile {
                target: "t".into(),
                path: PathBuf::from("/tmp/a b.png")
            }
        );
        assert!(parse(&["send-file", "t"]).is_err());
    }

    #[test]
    fn missing_or_extra_arguments_and_unknown_commands_are_errors() {
        assert!(parse(&["send", "only-target"]).is_err());
        assert!(parse(&["send", "a", "b", "extra"]).is_err());
        assert!(parse(&["group-add", "c", "t", "d", "a"]).is_err());
        assert!(parse(&["nope"]).is_err());
        assert!(parse(&[]).is_err());
        // The old standalone-core mailbox commands are gone, on purpose.
        assert!(parse(&["check-mailbox", "t"]).is_err());
        assert!(parse(&["check-mailbox-anon", "only-one"]).is_err());
        assert!(parse(&["listen"]).is_err());
    }

    #[test]
    fn text_starting_with_a_dash_needs_the_double_dash_separator() {
        assert!(parse(&["send", "t", "-hello"]).is_err());
        assert_eq!(
            parse(&["send", "t", "--", "-hello"]).unwrap().command,
            Command::Send {
                target: "t".into(),
                text: "-hello".into()
            }
        );
    }

    #[test]
    fn the_grammar_is_internally_consistent() {
        use clap::CommandFactory as _;
        Args::command().debug_assert();
    }
}
