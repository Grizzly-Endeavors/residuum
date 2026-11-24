pub mod clock;
pub mod gate;
pub mod log;
pub mod ports;
pub mod script;
pub mod wait;

use crate::build_info::BuildInfo;

pub const HARNESS_BUILD: BuildInfo = BuildInfo {
    version: "test",
    commit: None,
};
