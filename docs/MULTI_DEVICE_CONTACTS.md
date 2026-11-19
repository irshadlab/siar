# Multi-device contacts

Status: **built with the recommended options, in a simplified form** — see
"As built" immediately below; the original design follows unchanged.

## As built (verified by a three-core test)

A contact's second device is the same contact: Carol writes once and both of
Alice's devices receive it; Alice answering from her second device lands in
Carol's one chat with her.

* **Announcement** (`LinkMessage::DeviceAdded`): sent by the approving device to
  every paired contact when a device is approved, and again 25 s after each
  start-up (for contacts that were offline). The receiver accepts it only from
  an already-paired peer *and* only if the certificate verifies against the
  account root key, is for exactly the ticket it carries, has not expired, and
  the root key equals the one first seen for that contact (trust on first use;
  a changed root key is refused with a warning, never replaced).
  `siar-app-core::contact_devices`, 6 unit tests.
* **Storage: no migration.** Siblings live in `contact_devices.bin`. A
  conversation id is derived from the contact's primary ticket and never
  changes, so the alias table planned in step 2 was not needed.
* **Receiving:** a frame from a recorded sibling endpoint is routed to the
  contact's conversation and decrypted with the sibling's session; stored
  history opens messages with the primary's or any sibling's session.
* **Sending (fan-out), text only:** after the primary send succeeds, each
  sibling gets its own copy through the normal send path (own outbox row, retries
  and ACK tracking) filed under a hidden conversation, so the chat shows the
  message once. The delivery state shown is the primary device's.
* **Bootstrap:** at approval the person chooses "Approve" or "Approve and share
  my contacts" (`siar link-approve <id> --share-contacts`); shared contacts
  travel inside the link reply and are remembered in `imported_contacts.bin`.
* **Notices:** the contact is told "X added a new device"; a root-key change or
  an unverifiable announcement warns.

### Not built (and why it matters)

* **Revocation** (step 7): removing a device from a contact's record when it is
  revoked. Until then a sibling stays recorded.
* **Group messages and attachments are not fanned out** to a contact's extra
  devices; only 1:1 text is.
* **Alice's own sent messages are not mirrored to her other device**, so her
  second device's chat shows what the contact sent and what that device itself
  sent, not what her first device sent. That is self-sync (device-to-device
  history), a separate feature, as is history transfer.
* **Contacts added on the first device after linking are not pushed** to the
  second (only the contacts that existed at approval, if shared).
* **UI:** notices only. No device count on a contact, no per-device delivery
  detail, no dedicated security event.
* Imported contacts are titled by a short key id: a ticket carries no name.

## Original design

Earlier status line: Device *linking* works (see ROADMAP.md): a second
device can join an account and both list each other. What does not exist is
everything that makes a linked device *useful to other people*: your contacts
do not know it exists, so they cannot send to it and do not attribute its
messages to you. This note says what has to change and in what order, because
the work is a data-model change, not a notification.

## What the code does today (the constraints)

* A conversation id is derived from the peer's **ticket**
  (`peers::conversation_for`), and a ticket is one device (endpoint id +
  ed25519 + x25519 keys). One contact = one device = one conversation.
* An inbound frame is accepted only from an endpoint that is a *paired* peer
  (`peers.ticket_of_endpoint`); anything else is dropped (or, for group frames,
  recognised through the device directory).
* The transactional outbox is keyed by **message id**; `send_text` encrypts for
  one peer session and tracks one delivery state per message.
* A ticket does not carry a device id; the device id is learned from the
  peer's first envelope (`Envelope::sender`).
* Groups already identify members as (account, device) through MLS key
  packages, so groups are closer to multi-device than 1:1 chats are.

## What has to be true afterwards

1. **A contact is an account with N devices.** Alice's phone and desktop show
   as one contact and one conversation.
2. **Receiving:** a frame from any of Alice's known devices lands in Alice's
   conversation.
3. **Sending:** a message to Alice is delivered to *each* of her devices, with
   its own retry/delivery state per device; the conversation shows one message.
4. **Trust:** you learn about a new device of Alice's only through a path you
   already trust, and you are told when it happens.
5. **The new device works on day one:** it can message Alice's contacts without
   the person re-pairing everyone.

## Decisions needed (these are product/security choices, not implementation)

| Question | Option A (recommended) | Option B |
|---|---|---|
| How does Bob learn Alice's new device? | Alice's *existing* device announces it over the already-authenticated connection, with the certificate and the account root public key. Bob verifies the chain (root key → certificate → the device's ticket keys). | Bob must re-pair with every new device by ticket. Simple, no trust chain, but defeats linking for everyone but the owner. |
| Does Bob have to approve? | Notify only, and show a "Alice added a device" security event (and change the safety number display). Approval is optional per contact setting. | Always require approval. Safer, noisy. |
| How does the new device reach Alice's contacts? | The approving device sends the new one its contact tickets (inside the encrypted link channel) with the person's consent at approval time. | New device starts empty; person re-adds contacts. |
| History | Not synced in the first version; the new device starts with new messages. | Encrypted history transfer (a separate, large feature). |

Recommendation: A for all rows, history deferred.

## Plan (each step independently testable)

1. **Announcement message** (`LinkMessage::DeviceAdded` or its own wire kind):
   `{account, root_public_key, certificate, device ticket}` from the approving
   device to each contact, over the authenticated connection. Receiver checks
   the sender is a paired peer *and* that the certificate verifies against the
   root key *and* that the ticket's ed25519 key equals the certificate's device
   key. Pure verification function + unit tests first.
2. **Storage:** `contact_devices(contact, device_id, ticket, added_at)`.
   Conversation ids for existing contacts must not change: introduce an alias
   table (old ticket-derived id → contact id) in the same migration.
3. **Receive attribution:** frame from a known sibling endpoint → the contact's
   conversation. Update the admission standing lookup and the device directory
   the same way.
4. **Send fan-out:** one outbox row per (message id, device); the timeline shows
   one message whose state is the best of its devices (delivered if any device
   acknowledged, with a per-device detail view). This is the invasive part:
   the outbox schema and retry loop change.
5. **Bootstrap of the new device:** at approval, offer "share my contacts"; the
   contact tickets travel inside the link channel; the new device pairs with
   each (and each contact receives step 1's announcement).
6. **UI:** one contact row with a device count, a "devices" detail, the
   security event, and the per-device delivery detail.
7. **Revocation:** a revoked device is announced the same way (certificate
   generation increases) and removed from `contact_devices`.

## Why this was not built in the linking round

Steps 2 and 4 change the storage schema and the outbox that every message
depends on, and steps 1/5 change what a contact is allowed to do without a
person present. Doing that without the decisions above would have meant
guessing at the trust model. Step 1 is the right first piece once the table is
agreed: it is self-contained and can ship behind a flag.
