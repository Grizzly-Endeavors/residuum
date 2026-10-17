//! Pulse fires: one firing of a pulse, from its task scripts (if it has any)
//! through the session it starts (unless it is `script_only`), as the
//! gateway tracks it while it is in flight.
//!
//! The gateway publishes a [`PulseFireEvent`] on the system notification
//! channel each time a fire starts, moves on to its session, or finishes.
//! The web socket turns it into the `pulse_fire_changed` frame for the
//! Schedule place, and the agent's event loop records the outcome of a fire
//! that ended without a session run in `pulse_state.json`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::events::PulseOverlap;
use crate::types::SessionAddress;

/// Which part of its work a pulse fire is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PulseFirePhase {
    /// Its task scripts are running.
    Script,
    /// Its session has been asked to start, or is running.
    Session,
}

/// How a pulse fire ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PulseFireStatus {
    /// It finished normally: its scripts succeeded (a `script_only` fire),
    /// or its session's last turn finished.
    Ok,
    /// A script failed (a `script_only` fire), its session couldn't start,
    /// or its session's last turn failed.
    Failed,
    /// The owner stopped it.
    Stopped,
}

/// One pulse fire's outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PulseFireOutcome {
    pub status: PulseFireStatus,
    /// When the fire finished.
    pub at: DateTime<Utc>,
    /// What went wrong, in plain language, for a `failed` fire.
    #[serde(default)]
    pub error: Option<String>,
}

/// A pulse's run in flight, as the Schedule place shows it: a tracked fire,
/// or a live scheduled session of the pulse that no fire tracks (a session
/// resumed by a message to its address).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PulseFireRun {
    /// The fire's id, which its Stop endpoint takes. `None` for a session no
    /// fire tracks.
    pub fire_id: Option<String>,
    pub phase: PulseFirePhase,
    /// When the fire (or the untracked session) started.
    pub started_at: DateTime<Utc>,
    /// The fire's session address, once it has asked for its session.
    pub address: Option<SessionAddress>,
    /// The session's run id, once the session has registered.
    pub run_id: Option<String>,
    /// Set when the fire started while an earlier fire of the same pulse was
    /// still in flight.
    pub overlap: Option<PulseOverlap>,
}

/// What happened to a pulse fire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PulseFireChange {
    /// The fire started: its scripts are running, or (for a pulse without
    /// scripts) its session has been asked to start.
    Started,
    /// The fire's scripts finished and its session has been asked to start.
    SessionRequested,
    /// The fire's session registered and has a run id.
    SessionStarted,
    /// The fire finished.
    Finished {
        outcome: PulseFireOutcome,
        /// `true` when a session run ended the fire, so the session store
        /// already records this outcome; `false` for a fire that ended
        /// before or without one (a `script_only` fire, one stopped during
        /// its scripts, one whose session couldn't start), whose outcome
        /// the event loop records in `pulse_state.json`.
        session_run: bool,
    },
    /// The fire ended without an outcome to show: the agent is stopping, or
    /// the fire's task ended without finishing it.
    Ended,
}

/// A pulse fire changed. Carried on
/// [`super::topics::Notification`] (the system channel).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PulseFireEvent {
    /// The pulse's name.
    pub pulse: String,
    /// The fire that changed.
    pub fire_id: String,
    pub change: PulseFireChange,
    /// The pulse's run in flight after the change, as the Schedule place
    /// shows it: the newest fire of the pulse still in flight, which can be
    /// a different fire from `fire_id`. `None` when nothing of the pulse is
    /// running.
    pub current_run: Option<PulseFireRun>,
}
