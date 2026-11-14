//! `cargo edict`: deterministic architecture laws for Rust crates.

mod attrs;
mod cli;
mod config;
mod extract;
mod graph;
mod load;
mod model;
mod report;
mod resolve;
mod rules;

fn main() -> std::process::ExitCode {
    cli::main()
}
