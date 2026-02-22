//! Working indicator with animated dots and tool call counter.

use std::io::Write;

/// Animated working indicator displayed on stderr while the agent is processing.
///
/// Shows `Working.   (N tools)` with cycling dots, overwriting the same line.
pub struct WorkingIndicator {
    active: bool,
    tool_count: u32,
    dot_phase: u8,
}

impl WorkingIndicator {
    /// Create a new inactive indicator.
    #[must_use]
    pub fn new() -> Self {
        Self {
            active: false,
            tool_count: 0,
            dot_phase: 0,
        }
    }

    /// Start the indicator (called on `TurnStarted`).
    pub fn start(&mut self) {
        self.active = true;
        self.tool_count = 0;
        self.dot_phase = 0;
        self.redraw();
    }

    /// Record a tool call and redraw.
    pub fn on_tool_call(&mut self) {
        if self.active {
            self.tool_count = self.tool_count.saturating_add(1);
            self.redraw();
        }
    }

    /// Advance the dot animation and redraw. Called on a timer tick.
    pub fn tick(&mut self) {
        if self.active {
            self.dot_phase = (self.dot_phase + 1) % 4;
            self.redraw();
        }
    }

    /// Wipe the indicator line without deactivating.
    ///
    /// The animation resumes on the next tick or redraw. Use this when printing
    /// intermediate content while the agent is still working.
    pub fn clear_line(&mut self) {
        if self.active {
            eprint!("\r\x1b[2K");
            drop(std::io::stderr().flush());
        }
    }

    /// Clear the indicator line and mark inactive.
    pub fn finish(&mut self) {
        if self.active {
            self.active = false;
            eprint!("\r\x1b[2K");
            drop(std::io::stderr().flush());
        }
    }

    /// Whether the indicator is currently active (used as `tokio::select!` guard).
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active
    }

    fn redraw(&self) {
        let dots = match self.dot_phase {
            0 => "   ",
            1 => ".  ",
            2 => ".. ",
            _ => "...",
        };
        let tool_suffix = if self.tool_count > 0 {
            format!("  ({} tools)", self.tool_count)
        } else {
            String::new()
        };
        eprint!("\x1b[2K\rWorking{dots}{tool_suffix}");
        drop(std::io::stderr().flush());
    }
}

impl Default for WorkingIndicator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_inactive() {
        let ind = WorkingIndicator::new();
        assert!(!ind.is_active(), "new indicator should be inactive");
    }

    #[test]
    fn start_sets_active() {
        let mut ind = WorkingIndicator::new();
        ind.start();
        assert!(ind.is_active(), "start should set active");
        assert_eq!(ind.tool_count, 0, "start should reset tool count");
        assert_eq!(ind.dot_phase, 0, "start should reset dot phase");
    }

    #[test]
    fn on_tool_call_increments_count() {
        let mut ind = WorkingIndicator::new();
        ind.start();
        ind.on_tool_call();
        assert_eq!(ind.tool_count, 1, "tool count should be 1 after one call");
        ind.on_tool_call();
        assert_eq!(ind.tool_count, 2, "tool count should be 2 after two calls");
    }

    #[test]
    fn on_tool_call_noop_when_inactive() {
        let mut ind = WorkingIndicator::new();
        ind.on_tool_call();
        assert_eq!(
            ind.tool_count, 0,
            "tool count should not increment when inactive"
        );
    }

    #[test]
    fn tick_cycles_dot_phase() {
        let mut ind = WorkingIndicator::new();
        ind.start();
        assert_eq!(ind.dot_phase, 0, "should start at phase 0");
        ind.tick();
        assert_eq!(ind.dot_phase, 1, "should advance to phase 1");
        ind.tick();
        assert_eq!(ind.dot_phase, 2, "should advance to phase 2");
        ind.tick();
        assert_eq!(ind.dot_phase, 3, "should advance to phase 3");
        ind.tick();
        assert_eq!(ind.dot_phase, 0, "should wrap back to phase 0");
    }

    #[test]
    fn finish_clears_active() {
        let mut ind = WorkingIndicator::new();
        ind.start();
        ind.finish();
        assert!(!ind.is_active(), "finish should clear active state");
    }

    #[test]
    fn clear_line_preserves_active() {
        let mut ind = WorkingIndicator::new();
        ind.start();
        ind.on_tool_call();
        ind.clear_line();
        assert!(ind.is_active(), "clear_line should keep indicator active");
        assert_eq!(ind.tool_count, 1, "clear_line should not reset tool count");
    }

    #[test]
    fn clear_line_noop_when_inactive() {
        let mut ind = WorkingIndicator::new();
        ind.clear_line(); // should not panic
        assert!(!ind.is_active(), "should remain inactive");
    }

    #[test]
    fn finish_noop_when_inactive() {
        let mut ind = WorkingIndicator::new();
        ind.finish(); // should not panic
        assert!(!ind.is_active(), "should remain inactive");
    }
}
