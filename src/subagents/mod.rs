//! Subagent presets: disk-backed definitions for spawnable background agents.
//!
//! A subagent preset is a `.md` file with YAML frontmatter (name, description,
//! model tier, tool restrictions) plus a prompt body, discovered from a
//! presets directory. `SubagentPresetIndex` scans and looks up presets;
//! `SubagentRegistry` is the bus participant that subscribes to spawn
//! requests and turns a matched preset into a running background task.
//! Callers: `agent/context/loading.rs` and `tools/background.rs` scan the
//! index to list/validate presets, `background/spawn_context.rs` loads a
//! preset by name to spawn it, and `gateway/event_loop/run_loop.rs` builds
//! the `SubagentRegistry` and wires it onto the bus at startup.

pub mod index;
pub mod parser;
pub mod registry;
pub mod types;

pub use index::SubagentPresetIndex;
pub use registry::SubagentRegistry;
pub use types::SubagentPresetEntry;
