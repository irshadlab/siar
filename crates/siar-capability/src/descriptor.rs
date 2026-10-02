//! §7 "Capability Descriptor", §8 "Required vs Optional", §11-12
//! "Parameterized Capabilities" / "Capability Parameters", §17
//! "Durable vs Ephemeral Capabilities", §76 "Direction".

use crate::id::CapabilityId;
use crate::version::CapabilityVersion;
use serde::{Deserialize, Serialize};

/// §8: "Unknown required capability: negotiation failure. Unknown
/// optional capability: ignore safely." — the two variants exist
/// precisely so a negotiator can implement that rule by matching on
/// this field rather than guessing intent from context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CapabilityRequirement {
    Required,
    Optional,
}

/// §17: identity-bound capabilities live on the Part 02 device
/// certificate (durable), session-bound ones are re-negotiated every
/// session, and dynamic ones can change mid-session (§45, battery/
/// Wi-Fi/permission changes) without a full renegotiation. §16 is the
/// same distinction under different names ("Device Certificate vs
/// Runtime Capability") — this enum is the one place it's modeled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CapabilityLifetime {
    IdentityBound,
    SessionBound,
    Dynamic,
}

/// §76: "Some capabilities differ by direction... Do not represent as
/// one Boolean." Used for codecs (encode vs decode), file transfer
/// (upload vs download), and relay capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CapabilityDirection {
    Send,
    Receive,
    Both,
}

/// Cap chosen for [`BoundedBytes`] and [`CapabilityParameters::Bytes`].
///
/// §61 requires *an* explicit "max bytes" bound on advertisement
/// payloads but never states the number — deferred to whichever crate
/// eventually owns the wire advertisement size budget (§101-102, not
/// built this pass). 256 bytes is chosen here only as this type's own
/// internal ceiling, generous enough for any single parameter (a
/// certificate fingerprint, a short capability blob) while still
/// making "unbounded" impossible to construct.
pub const MAX_PARAMETER_BYTES: usize = 256;

/// §12: a bounded byte parameter — rejects at construction rather than
/// truncating silently, matching this workspace's established
/// bounded-collection pattern (`siar_protocol_ext::BoundedQueue`).
///
/// Deserialization goes through [`BoundedBytes::new`] (`try_from`), so
/// the bound also holds for bytes a *peer* sent — a plain derived
/// `Deserialize` would let a hostile advertisement carry an arbitrarily
/// large parameter straight past the constructor's check.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "Vec<u8>", into = "Vec<u8>")]
pub struct BoundedBytes(Vec<u8>);

impl TryFrom<Vec<u8>> for BoundedBytes {
    type Error = BoundedBytesError;
    fn try_from(v: Vec<u8>) -> Result<Self, Self::Error> {
        Self::new(v)
    }
}

impl From<BoundedBytes> for Vec<u8> {
    fn from(b: BoundedBytes) -> Self {
        b.0
    }
}

impl BoundedBytes {
    pub fn new(bytes: Vec<u8>) -> Result<Self, BoundedBytesError> {
        if bytes.len() > MAX_PARAMETER_BYTES {
            return Err(BoundedBytesError::TooLarge {
                len: bytes.len(),
                max: MAX_PARAMETER_BYTES,
            });
        }
        Ok(Self(bytes))
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BoundedBytesError {
    #[error("parameter bytes too large: {len} > max {max}")]
    TooLarge { len: usize, max: usize },
}

/// §12: a small fixed bitset for common boolean-flag capability groups
/// (§62's `MessagingCapabilityBits`, `FilesCapabilityBits` examples).
/// `u64` gives 64 flag slots per capability group, evolved per §63
/// ("reserve bits carefully... do not reuse removed bits") — enforcing
/// the non-reuse rule itself is a registry/documentation concern, not
/// something a plain bitset type can check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(transparent)]
pub struct CapabilityBits(pub u64);

impl CapabilityBits {
    pub const EMPTY: Self = Self(0);

    /// Sets `bit`. Panics if `bit >= 64`: the previous `bit & 63` masking
    /// silently aliased bit 64 onto bit 0, i.e. granting or testing a
    /// *different* capability than the caller named. Use
    /// [`Self::try_with_bit`] for any index not known at compile time.
    pub fn with_bit(self, bit: u8) -> Self {
        self.try_with_bit(bit)
            .expect("CapabilityBits supports bit indices 0..=63")
    }

    pub fn try_with_bit(self, bit: u8) -> Option<Self> {
        (bit < 64).then(|| Self(self.0 | (1u64 << bit)))
    }

    /// Out-of-range indices are simply "not set".
    pub fn has_bit(self, bit: u8) -> bool {
        bit < 64 && (self.0 & (1u64 << bit)) != 0
    }

    /// §19's simple-Boolean intersection rule (`negotiated = local ∩
    /// remote`), applied bit-by-bit to a whole flag group at once.
    pub fn intersect(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
}

/// §12: "Do not use `HashMap<String, String>` for core behavior." —
/// this closed enum is the alternative: every shape a capability
/// parameter can take is a distinct, typed variant. `RangeU32` backs
/// §19's range-intersection rule (`overlap(local, remote)`) and
/// `U32`/`U64` back its max-limit rule (`min(local_max, remote_max,
/// policy_max)`); neither is implemented as generic arithmetic here
/// since the correct combining rule depends on which one the caller
/// means (negotiation logic owns that, not this type).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CapabilityParameters {
    None,
    U32(u32),
    U64(u64),
    RangeU32 { min: u32, max: u32 },
    BitSet(CapabilityBits),
    Bytes(BoundedBytes),
}

/// §7: one advertised capability — its identity, its own version, how
/// it's negotiated (required/optional), and its typed parameters.
/// `stability` is named in §7's sketch (`CapabilityStability`) but
/// left with no defined variants anywhere else in the spec — omitted
/// here rather than guessed at; adding it later is additive, not a
/// breaking change to this struct's other fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityDescriptor {
    pub id: CapabilityId,
    pub version: CapabilityVersion,
    pub requirement: CapabilityRequirement,
    pub parameters: CapabilityParameters,
}

impl CapabilityDescriptor {
    pub const fn new(
        id: CapabilityId,
        version: CapabilityVersion,
        requirement: CapabilityRequirement,
        parameters: CapabilityParameters,
    ) -> Self {
        Self {
            id,
            version,
            requirement,
            parameters,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn out_of_range_bits_do_not_alias_low_bits() {
        let b = CapabilityBits::EMPTY.with_bit(0);
        assert!(!b.has_bit(64), "bit 64 must not read as bit 0");
        assert_eq!(CapabilityBits::EMPTY.try_with_bit(64), None);
        assert!(CapabilityBits::EMPTY.try_with_bit(63).unwrap().has_bit(63));
    }

    #[test]
    fn bounded_bytes_bound_also_holds_on_the_wire() {
        let wire = postcard::to_allocvec(&vec![0u8; MAX_PARAMETER_BYTES + 1]).unwrap();
        assert!(postcard::from_bytes::<BoundedBytes>(&wire).is_err());
        let ok = postcard::to_allocvec(&vec![7u8; 4]).unwrap();
        assert_eq!(
            postcard::from_bytes::<BoundedBytes>(&ok)
                .unwrap()
                .as_slice(),
            &[7, 7, 7, 7]
        );
    }

    #[test]
    fn bounded_bytes_rejects_oversized_input() {
        let ok = BoundedBytes::new(vec![0u8; MAX_PARAMETER_BYTES]);
        assert!(ok.is_ok());

        let too_big = BoundedBytes::new(vec![0u8; MAX_PARAMETER_BYTES + 1]);
        assert_eq!(
            too_big.unwrap_err(),
            BoundedBytesError::TooLarge {
                len: MAX_PARAMETER_BYTES + 1,
                max: MAX_PARAMETER_BYTES
            }
        );
    }

    #[test]
    fn bits_intersect_matches_boolean_and() {
        let local = CapabilityBits::EMPTY.with_bit(0).with_bit(2).with_bit(5);
        let remote = CapabilityBits::EMPTY.with_bit(2).with_bit(5).with_bit(9);
        let negotiated = local.intersect(remote);

        assert!(!negotiated.has_bit(0)); // local-only
        assert!(negotiated.has_bit(2)); // shared
        assert!(negotiated.has_bit(5)); // shared
        assert!(!negotiated.has_bit(9)); // remote-only
    }
}
