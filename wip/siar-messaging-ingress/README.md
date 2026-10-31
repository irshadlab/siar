# UNFINISHED, NOT BUILT, NOT TESTED — set aside, not part of the workspace

An interrupted attempt to (1) carry a signed-prekey bundle in `PeerTicket`
(`ticket.rs.with-bundle-extension`) and (2) put `siar-abuse`'s gate in front of
`MessageService::handle_incoming` (`ingress.rs`). The script died before the
`lib.rs`/`service.rs` edits, and left a duplicate `blake3` key in Cargo.toml
(kept here only for the record). None of this compiled or ran.

Design notes worth keeping if resumed:
* extension block after the v1 ticket body (tag 0xB1) so old readers still work;
* the gate must NOT deduplicate message ids — a retransmission after a crash
  must still be accepted; storage-level `insert_if_new` is the right dedupe.

Whether to resume depends on the MLS-vs-ratchet decision (see discussion): if
1:1 moves to OpenMLS, the bundle becomes an MLS KeyPackage.
