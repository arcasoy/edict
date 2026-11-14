//! The laws. Each rule is a pure function of the analysis and the config.
//!
//! Children never reach back into this file: shared types live in
//! `crate::model`, so the rules directory is itself cycle-free.

mod cycles;
mod facade;
mod global_state;
mod layers;
mod limits;
mod purity;

use crate::config::Config;
use crate::model::{Analysis, Violation};

pub fn run(analysis: &Analysis, cfg: &Config) -> Vec<Violation> {
    let mut out = Vec::new();
    out.extend(cycles::check(analysis, cfg));
    out.extend(layers::check(analysis, cfg));
    out.extend(facade::check(analysis, cfg));
    out.extend(purity::check(analysis, cfg));
    out.extend(global_state::check(analysis, cfg));
    out.extend(limits::check(analysis, cfg));
    out
}
