//! Linking a new device to an existing account — the part that needs no
//! network: who may link, with what proof, for how long, and what the
//! approving device signs.
//!
//! The flow this engine serves:
//!
//! 1. **Offer.** An existing device calls [`LinkOffers::new_offer`] and shows
//!    the returned one-time code (with its own ticket) to the person.
//! 2. **Request.** The new device sends its ids and public key plus that code;
//!    the existing device calls [`LinkOffers::submit`]. The code is checked in
//!    constant time, expires after [`OFFER_TTL`], allows [`MAX_ATTEMPTS`]
//!    wrong tries in total, and is **consumed by the first correct use**.
//! 3. **Approval.** The request waits as *pending*. Nothing is issued until a
//!    person approves it on the existing device ([`LinkOffers::approve`]), who
//!    sees the new device's name and key fingerprint first. Approval signs a
//!    [`DeviceCertificate`] with the account's root key.
//! 4. **Acceptance.** The new device checks the certificate with
//!    [`accept_certificate`] against the root public key it received over the
//!    channel it already authenticated, and against its *own* device id and
//!    key — a certificate for some other device is refused.
//!
//! Not here (and not claimed): the wire messages, the transport, the UI, and
//! persisting the root key (`RootIdentityKey` has no export/import yet).
//! Those are wiring, built on this.

use std::time::Duration;

use siar_domain::{AccountId, DeviceId};
use siar_identity_multidevice::capability::DeviceCapabilitySet;
use siar_identity_multidevice::certificate::DeviceCertificate;
use siar_identity_multidevice::linking_authority::DeviceRole;
use siar_identity_multidevice::root_key::{RootIdentityKey, RootPublicKey};

/// How long a one-time code can be used.
pub const OFFER_TTL: Duration = Duration::from_secs(5 * 60);
/// Wrong codes tolerated for one offer before it is destroyed.
pub const MAX_ATTEMPTS: u8 = 5;
/// How long a verified request waits for a person to approve it.
pub const PENDING_TTL: Duration = Duration::from_secs(10 * 60);
/// At most this many requests wait at once.
pub const MAX_PENDING: usize = 4;
const MAX_NAME_CHARS: usize = 64;

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LinkError {
    #[error("there is no link offer open (or it expired)")]
    NoOffer,
    #[error("that code is not right")]
    WrongCode,
    #[error("too many wrong codes; start a new link offer")]
    OfferExhausted,
    #[error("too many requests are waiting for approval")]
    TooManyPending,
    #[error("that device is already waiting")]
    DuplicateDevice,
    #[error("that device's key is not valid")]
    BadKey,
    #[error("no such waiting request (or it expired)")]
    NoSuchRequest,
    #[error("the certificate is not valid for this account, device or time")]
    BadCertificate,
}

/// What a new device presents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkRequest {
    pub device_id: DeviceId,
    pub device_public_key: [u8; 32],
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingLink {
    pub request: LinkRequest,
    /// First 8 hex characters of the BLAKE3 hash of the device key — what the
    /// person compares on both screens before approving.
    pub fingerprint: String,
    pub expires_at_millis: u64,
}

struct Offer {
    code_hash: blake3::Hash,
    expires_at_millis: u64,
    attempts_left: u8,
}

#[derive(Default)]
pub struct LinkOffers {
    offer: Option<Offer>,
    pending: Vec<PendingLink>,
}

fn encode_code(bytes: &[u8; 10]) -> String {
    let mut bits = 0u32;
    let mut have = 0u32;
    let mut out = String::with_capacity(19);
    for &b in bytes {
        bits = (bits << 8) | u32::from(b);
        have += 8;
        while have >= 5 {
            have -= 5;
            let idx = ((bits >> have) & 0x1f) as usize;
            if !out.is_empty() && out.chars().filter(|c| *c != '-').count() % 4 == 0 {
                out.push('-');
            }
            out.push(ALPHABET[idx] as char);
        }
    }
    out
}

/// Normalise what a person typed: case-insensitive, separators ignored, and
/// the look-alikes Crockford base32 folds (`O`→`0`, `I`/`L`→`1`).
fn normalise(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            u => u,
        })
        .collect()
}

fn hash_of(code: &str) -> blake3::Hash {
    blake3::hash(normalise(code).as_bytes())
}

fn fingerprint(key: &[u8; 32]) -> String {
    blake3::hash(key).to_hex()[..8].to_owned()
}

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

impl LinkOffers {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Open a new one-time offer (replacing any earlier one) and return the
    /// code to show, e.g. `7K2D-9QWM-X4PT-H8ZB`.
    pub fn new_offer(&mut self, now_millis: u64) -> String {
        let mut raw = [0u8; 10];
        // `uuid` draws from the OS CSPRNG (already a dependency).
        raw.copy_from_slice(&uuid::Uuid::new_v4().as_bytes()[..10]);
        let code = encode_code(&raw);
        self.offer = Some(Offer {
            code_hash: hash_of(&code),
            expires_at_millis: now_millis.saturating_add(millis(OFFER_TTL)),
            attempts_left: MAX_ATTEMPTS,
        });
        code
    }

    /// Whether a code is currently usable (open, not expired).
    #[must_use]
    pub fn has_open_offer(&self, now_millis: u64) -> bool {
        self.offer
            .as_ref()
            .is_some_and(|o| o.expires_at_millis > now_millis)
    }

    /// Check a request's code. On success the offer is consumed and the
    /// request waits for approval; the returned `PendingLink` is what to show.
    pub fn submit(
        &mut self,
        now_millis: u64,
        code: &str,
        request: LinkRequest,
    ) -> Result<PendingLink, LinkError> {
        self.pending.retain(|p| p.expires_at_millis > now_millis);
        let offer = self.offer.as_mut().ok_or(LinkError::NoOffer)?;
        if offer.expires_at_millis <= now_millis {
            self.offer = None;
            return Err(LinkError::NoOffer);
        }
        // `blake3::Hash` compares in constant time.
        if hash_of(code) != offer.code_hash {
            offer.attempts_left -= 1;
            if offer.attempts_left == 0 {
                self.offer = None;
                return Err(LinkError::OfferExhausted);
            }
            return Err(LinkError::WrongCode);
        }
        if request.device_public_key == [0u8; 32] {
            return Err(LinkError::BadKey); // right code, but keep the offer
        }
        if self.pending.len() >= MAX_PENDING {
            return Err(LinkError::TooManyPending);
        }
        if self
            .pending
            .iter()
            .any(|p| p.request.device_id == request.device_id)
        {
            return Err(LinkError::DuplicateDevice);
        }
        self.offer = None; // single use
        let pending = PendingLink {
            fingerprint: fingerprint(&request.device_public_key),
            expires_at_millis: now_millis.saturating_add(millis(PENDING_TTL)),
            request: LinkRequest {
                display_name: request
                    .display_name
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(MAX_NAME_CHARS)
                    .collect(),
                ..request
            },
        };
        self.pending.push(pending.clone());
        Ok(pending)
    }

    #[must_use]
    pub fn pending(&self, now_millis: u64) -> Vec<&PendingLink> {
        self.pending
            .iter()
            .filter(|p| p.expires_at_millis > now_millis)
            .collect()
    }

    /// A person approved `device`: sign its certificate with the account's
    /// root key. The request is removed whether or not signing is reached.
    pub fn approve(
        &mut self,
        device: DeviceId,
        now_millis: u64,
        root: &RootIdentityKey,
        account: AccountId,
        generation: u64,
    ) -> Result<DeviceCertificate, LinkError> {
        let i = self
            .pending
            .iter()
            .position(|p| p.request.device_id == device && p.expires_at_millis > now_millis)
            .ok_or(LinkError::NoSuchRequest)?;
        let p = self.pending.remove(i);
        Ok(DeviceCertificate::issue(
            root,
            account,
            p.request.device_id,
            p.request.device_public_key,
            now_millis,
            None,
            DeviceRole::Standard.default_capabilities(),
            generation,
        ))
    }

    /// A person declined: forget the request.
    pub fn reject(&mut self, device: DeviceId) -> bool {
        let before = self.pending.len();
        self.pending.retain(|p| p.request.device_id != device);
        self.pending.len() != before
    }
}

/// The new device's check of the certificate it was sent.
pub fn accept_certificate(
    root_public_key: &RootPublicKey,
    certificate: &DeviceCertificate,
    my_device: DeviceId,
    my_public_key: &[u8; 32],
    now_millis: u64,
) -> Result<(), LinkError> {
    if certificate.device_id != my_device
        || &certificate.device_public_key != my_public_key
        || certificate.is_expired(now_millis)
        || !certificate
            .capabilities
            .contains(DeviceCapabilitySet::SEND_MESSAGE)
    {
        return Err(LinkError::BadCertificate);
    }
    certificate
        .verify_signature(root_public_key)
        .map_err(|_| LinkError::BadCertificate)
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: u64 = 1_000_000;

    fn req(n: u8) -> LinkRequest {
        LinkRequest {
            device_id: DeviceId::new(),
            device_public_key: [n; 32],
            display_name: format!("phone {n}"),
        }
    }

    #[test]
    fn codes_look_like_groups_of_four_and_parse_forgivingly() {
        let mut o = LinkOffers::new();
        let c = o.new_offer(T0);
        assert_eq!(c.len(), 19, "{c}");
        assert_eq!(c.matches('-').count(), 3);
        assert!(c
            .chars()
            .all(|ch| ch == '-' || ALPHABET.contains(&(ch as u8))));
        // Lower case, no dashes, stray spaces: all fine.
        let sloppy = c.to_lowercase().replace('-', " ");
        assert!(o.submit(T0 + 1, &sloppy, req(1)).is_ok());
    }

    #[test]
    fn look_alike_letters_are_folded() {
        assert_eq!(normalise("o0-Il1"), "00111");
    }

    #[test]
    fn a_code_works_once() {
        let mut o = LinkOffers::new();
        let c = o.new_offer(T0);
        assert!(o.submit(T0, &c, req(1)).is_ok());
        assert_eq!(o.submit(T0, &c, req(2)), Err(LinkError::NoOffer));
    }

    #[test]
    fn a_code_expires() {
        let mut o = LinkOffers::new();
        let c = o.new_offer(T0);
        let late = T0 + millis(OFFER_TTL);
        assert_eq!(o.submit(late, &c, req(1)), Err(LinkError::NoOffer));
    }

    #[test]
    fn five_wrong_codes_destroy_the_offer_even_for_the_right_code_after() {
        let mut o = LinkOffers::new();
        let good = o.new_offer(T0);
        for i in 1..MAX_ATTEMPTS {
            assert_eq!(
                o.submit(T0, "AAAA-AAAA-AAAA-AAAA", req(i)),
                Err(LinkError::WrongCode)
            );
        }
        assert_eq!(
            o.submit(T0, "AAAA-AAAA-AAAA-AAAA", req(9)),
            Err(LinkError::OfferExhausted)
        );
        assert_eq!(o.submit(T0, &good, req(1)), Err(LinkError::NoOffer));
    }

    #[test]
    fn a_new_offer_replaces_the_old_one() {
        let mut o = LinkOffers::new();
        let first = o.new_offer(T0);
        let second = o.new_offer(T0);
        assert_ne!(first, second);
        assert_eq!(o.submit(T0, &first, req(1)), Err(LinkError::WrongCode));
        assert!(o.submit(T0, &second, req(1)).is_ok());
    }

    #[test]
    fn nothing_is_issued_until_a_person_approves_and_approval_signs_for_that_device() {
        let root = RootIdentityKey::generate();
        let account = AccountId::new();
        let mut o = LinkOffers::new();
        let c = o.new_offer(T0);
        let r = req(7);
        let device = r.device_id;
        let pending = o.submit(T0, &c, r.clone()).unwrap();
        assert_eq!(pending.fingerprint.len(), 8);
        assert_eq!(o.pending(T0).len(), 1);

        let cert = o.approve(device, T0 + 5, &root, account, 1).unwrap();
        assert_eq!((cert.account_id, cert.device_id), (account, device));
        assert!(o.pending(T0 + 5).is_empty());
        // The new device accepts it with the root public key it was given.
        assert_eq!(
            accept_certificate(
                &root.root_public_key(),
                &cert,
                device,
                &r.device_public_key,
                T0 + 6
            ),
            Ok(())
        );
        // Approving twice is not possible.
        assert_eq!(
            o.approve(device, T0 + 6, &root, account, 1).unwrap_err(),
            LinkError::NoSuchRequest
        );
    }

    #[test]
    fn a_certificate_for_another_device_key_account_root_or_time_is_refused() {
        let root = RootIdentityKey::generate();
        let other_root = RootIdentityKey::generate();
        let mut o = LinkOffers::new();
        let c = o.new_offer(T0);
        let r = req(3);
        let (device, key) = (r.device_id, r.device_public_key);
        o.submit(T0, &c, r).unwrap();
        let cert = o.approve(device, T0, &root, AccountId::new(), 1).unwrap();
        let pk = root.root_public_key();
        // Someone else's device id, someone else's key, the wrong root.
        assert_eq!(
            accept_certificate(&pk, &cert, DeviceId::new(), &key, T0),
            Err(LinkError::BadCertificate)
        );
        assert_eq!(
            accept_certificate(&pk, &cert, device, &[9; 32], T0),
            Err(LinkError::BadCertificate)
        );
        assert_eq!(
            accept_certificate(&other_root.root_public_key(), &cert, device, &key, T0),
            Err(LinkError::BadCertificate)
        );
        // A tampered certificate (different key, same signature) fails.
        let mut forged = cert.clone();
        forged.device_public_key = [9; 32];
        assert_eq!(
            accept_certificate(&pk, &forged, device, &[9; 32], T0),
            Err(LinkError::BadCertificate)
        );
    }

    #[test]
    fn waiting_requests_are_bounded_unique_and_expire() {
        let mut o = LinkOffers::new();
        let mut devices = Vec::new();
        for n in 1..=MAX_PENDING as u8 {
            let c = o.new_offer(T0);
            let r = req(n);
            devices.push(r.device_id);
            o.submit(T0, &c, r).unwrap();
        }
        let c = o.new_offer(T0);
        assert_eq!(o.submit(T0, &c, req(50)), Err(LinkError::TooManyPending));
        // After the wait runs out, room opens up again.
        let later = T0 + millis(PENDING_TTL) + 1;
        let c = o.new_offer(later);
        assert!(o.submit(later, &c, req(51)).is_ok());
        assert_eq!(o.pending(later).len(), 1);
        // Declining removes a request.
        let d = o.pending(later)[0].request.device_id;
        assert!(o.reject(d));
        assert!(!o.reject(d));
    }

    #[test]
    fn a_duplicate_device_and_a_zero_key_are_refused_without_burning_the_offer() {
        let mut o = LinkOffers::new();
        let c = o.new_offer(T0);
        let mut zero = req(1);
        zero.device_public_key = [0; 32];
        assert_eq!(o.submit(T0, &c, zero), Err(LinkError::BadKey));
        // The right code with a bad key did not consume the offer.
        let r = req(2);
        let id = r.device_id;
        o.submit(T0, &c, r.clone()).unwrap();
        let c2 = o.new_offer(T0);
        assert_eq!(o.submit(T0, &c2, r), Err(LinkError::DuplicateDevice));
        assert_eq!(o.pending(T0)[0].request.device_id, id);
    }

    #[test]
    fn hostile_display_names_are_cleaned() {
        let mut o = LinkOffers::new();
        let c = o.new_offer(T0);
        let mut r = req(1);
        r.display_name = format!("evil\u{1b}[31m{}", "x".repeat(500));
        let p = o.submit(T0, &c, r).unwrap();
        assert!(!p.request.display_name.contains('\u{1b}'));
        assert_eq!(p.request.display_name.chars().count(), MAX_NAME_CHARS);
    }
}
