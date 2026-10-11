//! Endpoint capability flags for the bus.

// ---------------------------------------------------------------------------
// EndpointCapabilities
// ---------------------------------------------------------------------------

/// Bitfield describing what an endpoint can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EndpointCapabilities(u8);

impl EndpointCapabilities {
    /// Bidirectional conversation.
    pub const INTERACTIVE: Self = Self(0b0001);
    /// Output-only push (notifications).
    pub const NOTIFY_ONLY: Self = Self(0b0100);

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
    use super::*;

    #[test]
    fn capabilities_empty_contains_nothing() {
        let empty = EndpointCapabilities::empty();
        assert!(!empty.contains(EndpointCapabilities::INTERACTIVE));
        assert!(!empty.contains(EndpointCapabilities::NOTIFY_ONLY));
    }

    #[test]
    fn capabilities_union_and_contains() {
        let caps = EndpointCapabilities::INTERACTIVE.union(EndpointCapabilities::NOTIFY_ONLY);
        assert!(caps.contains(EndpointCapabilities::INTERACTIVE));
        assert!(caps.contains(EndpointCapabilities::NOTIFY_ONLY));
        assert!(!EndpointCapabilities::INTERACTIVE.contains(EndpointCapabilities::NOTIFY_ONLY));
    }

    #[test]
    fn all_flags_are_distinct() {
        use super::EndpointCapabilities as C;
        assert_ne!(C::INTERACTIVE, C::NOTIFY_ONLY);
    }

    #[test]
    fn capabilities_contains_empty() {
        assert!(EndpointCapabilities::INTERACTIVE.contains(EndpointCapabilities::empty()));
        assert!(EndpointCapabilities::empty().contains(EndpointCapabilities::empty()));
    }
}
