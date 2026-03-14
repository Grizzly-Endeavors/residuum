//! Endpoint identity and capability flags for the bus.

use std::fmt;

// ---------------------------------------------------------------------------
// EndpointId
// ---------------------------------------------------------------------------

/// Unique identifier for a bus endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EndpointId(String);

impl fmt::Display for EndpointId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for EndpointId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<&str> for EndpointId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl From<String> for EndpointId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

// ---------------------------------------------------------------------------
// EndpointCapabilities
// ---------------------------------------------------------------------------

/// Bitfield describing what an endpoint can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EndpointCapabilities(u8);

impl EndpointCapabilities {
    /// Bidirectional conversation.
    pub const INTERACTIVE: Self = Self(0b0001);
    /// Supports streaming events (typing indicators, tool calls).
    pub const STREAMING: Self = Self(0b0010);
    /// Output-only push (notifications).
    pub const NOTIFY_ONLY: Self = Self(0b0100);
    /// Input-only, no response path.
    pub const INPUT_ONLY: Self = Self(0b1000);

    /// No capabilities set.
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Combine two capability sets.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Check whether all bits in `other` are present in `self`.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn endpoint_id_equality_and_hash() {
        let a = EndpointId::from("ws");
        let b = EndpointId::from("ws");
        assert_eq!(a, b);

        let mut set = HashSet::new();
        set.insert(a.clone());
        assert!(set.contains(&b));
    }

    #[test]
    fn endpoint_id_display() {
        let id = EndpointId::from("telegram");
        assert_eq!(id.to_string(), "telegram");
    }

    #[test]
    fn capabilities_empty_contains_nothing() {
        let empty = EndpointCapabilities::empty();
        assert!(!empty.contains(EndpointCapabilities::INTERACTIVE));
        assert!(!empty.contains(EndpointCapabilities::STREAMING));
        assert!(!empty.contains(EndpointCapabilities::NOTIFY_ONLY));
        assert!(!empty.contains(EndpointCapabilities::INPUT_ONLY));
    }

    #[test]
    fn capabilities_union_and_contains() {
        let caps = EndpointCapabilities::INTERACTIVE.union(EndpointCapabilities::STREAMING);
        assert!(caps.contains(EndpointCapabilities::INTERACTIVE));
        assert!(caps.contains(EndpointCapabilities::STREAMING));
        assert!(!caps.contains(EndpointCapabilities::NOTIFY_ONLY));
        assert!(!caps.contains(EndpointCapabilities::INPUT_ONLY));
    }

    #[test]
    fn all_flags_are_distinct() {
        let flags = [
            EndpointCapabilities::INTERACTIVE,
            EndpointCapabilities::STREAMING,
            EndpointCapabilities::NOTIFY_ONLY,
            EndpointCapabilities::INPUT_ONLY,
        ];
        for (i, a) in flags.iter().enumerate() {
            for (j, b) in flags.iter().enumerate() {
                if i == j {
                    assert_eq!(a, b);
                } else {
                    assert_ne!(a, b);
                }
            }
        }
    }
}
