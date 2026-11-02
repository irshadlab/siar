//! Command-line grammar for `siar` (clap derive). Kept apart from `main.rs`
//! so the grammar can be unit-tested without touching the network.
//!
//! Usage text, `--help`, `--version` and "missing argument" errors all come
//! from clap now; the hand-written usage strings are gone.

use clap::{Parser, Subcommand};

/// Siar acceptance-harness CLI (plan.md section 98) — not a production client.
#[derive(Debug, Parser)]
#[command(name = "siar", version, about)]
pub struct Args {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, PartialEq, Eq, Subcommand)]
pub enum Command {
    /// Listen for incoming messages; optionally pre-pair with a peer's ticket.
    ///
    /// The ticket tells `listen` which X25519 key to decrypt incoming frames
    /// with (hand-pairing, Phase 1 only — real contact discovery is later).
    Listen {
        /// Publish a key package and prompt to add group members in-loop.
        #[arg(long)]
        publish_key_package: bool,
        /// The peer's ticket.
        their_ticket: Option<String>,
    },
    /// Send one message and exit.
    Send { their_ticket: String, text: String },
    /// Send a file as an attachment.
    SendFile { their_ticket: String, path: String },
    /// Print a key package (+ device/account ids) for a group admin to add.
    PublishKeyPackage,
    /// Fetch queued messages from a relay.
    CheckMailbox { relay_ticket: String },
    /// Deliver via the unlinkable token-mailbox path (see `send_text_anon`'s
    /// docs for what this does and does not guarantee).
    SendAnon {
        their_ticket: String,
        relay_ticket: String,
        text: String,
    },
    /// Check for messages sent with `send-anon` from one specific peer (an
    /// anonymous check-in has no sender field, so you must name who).
    CheckMailboxAnon {
        their_ticket: String,
        relay_ticket: String,
    },
    /// Create a new MLS group.
    GroupCreate,
    /// Add a member (run `publish-key-package` in the peer's process first to
    /// get the last three values).
    GroupAddMember {
        conversation_id: String,
        peer_ticket: String,
        peer_device_id: String,
        peer_account_id: String,
        base64_key_package: String,
    },
    /// Send a message to a group.
    GroupSend {
        conversation_id: String,
        text: String,
    },
    /// Join from welcome bytes. Not independently usable as a separate process
    /// today — see this crate's module docs; kept for scripting.
    JoinGroup {
        conversation_id: String,
        base64_welcome_bytes: String,
        base64_group_state: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(a: &[&str]) -> Result<Command, clap::Error> {
        Args::try_parse_from(std::iter::once("siar").chain(a.iter().copied())).map(|a| a.command)
    }

    #[test]
    fn listen_flag_may_come_before_or_after_the_ticket() {
        let want = Command::Listen {
            publish_key_package: true,
            their_ticket: Some("t".into()),
        };
        assert_eq!(
            parse(&["listen", "--publish-key-package", "t"]).unwrap(),
            want
        );
        assert_eq!(
            parse(&["listen", "t", "--publish-key-package"]).unwrap(),
            want
        );
        assert_eq!(
            parse(&["listen"]).unwrap(),
            Command::Listen {
                publish_key_package: false,
                their_ticket: None
            }
        );
    }

    #[test]
    fn positional_commands_parse_in_order() {
        assert_eq!(
            parse(&["send", "tick", "hello world"]).unwrap(),
            Command::Send {
                their_ticket: "tick".into(),
                text: "hello world".into()
            }
        );
        assert_eq!(
            parse(&["send-anon", "p", "r", "hi"]).unwrap(),
            Command::SendAnon {
                their_ticket: "p".into(),
                relay_ticket: "r".into(),
                text: "hi".into()
            }
        );
        assert_eq!(
            parse(&["group-add-member", "c", "t", "d", "a", "kp"]).unwrap(),
            Command::GroupAddMember {
                conversation_id: "c".into(),
                peer_ticket: "t".into(),
                peer_device_id: "d".into(),
                peer_account_id: "a".into(),
                base64_key_package: "kp".into(),
            }
        );
    }

    #[test]
    fn missing_or_extra_arguments_and_unknown_commands_are_errors() {
        assert!(parse(&["send", "only-ticket"]).is_err());
        assert!(parse(&["send", "a", "b", "extra"]).is_err());
        assert!(parse(&["group-add-member", "c", "t", "d", "a"]).is_err());
        assert!(parse(&["nope"]).is_err());
        assert!(parse(&[]).is_err());
    }

    #[test]
    fn text_starting_with_a_dash_needs_the_double_dash_separator() {
        assert!(parse(&["send", "t", "-hello"]).is_err());
        assert_eq!(
            parse(&["send", "t", "--", "-hello"]).unwrap(),
            Command::Send {
                their_ticket: "t".into(),
                text: "-hello".into()
            }
        );
    }

    #[test]
    fn help_and_version_are_clap_exits_not_failures() {
        assert_eq!(
            parse(&["--help"]).unwrap_err().kind(),
            clap::error::ErrorKind::DisplayHelp
        );
        assert_eq!(
            parse(&["--version"]).unwrap_err().kind(),
            clap::error::ErrorKind::DisplayVersion
        );
    }

    #[test]
    fn the_grammar_is_internally_consistent() {
        use clap::CommandFactory as _;
        Args::command().debug_assert();
    }
}
