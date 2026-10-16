//! The version and commit the binary was built from.

/// Build identity of the running binary.
///
/// The root crate builds the one value a process uses (`residuum::BUILD_INFO`),
/// from the variables its build script injects. Everything that reports or
/// compares the running version takes a `BuildInfo` instead of reading those
/// variables itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildInfo {
    /// Release version, as tagged (for example `2026.03.02`).
    pub version: &'static str,
    /// Git commit the binary was built from, when the build recorded one.
    pub commit: Option<&'static str>,
}
