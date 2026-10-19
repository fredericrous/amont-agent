//! amont-agent's guard: what an agent's tool call may do. Shell analysis,
//! the rules and their stances, the journal, and the utilities both
//! components share. It never names the work process (`amont-agent-process`)
//! or the binary: the crate graph keeps it that way (decisions ADR-0039;
//! plan `docs/plans/2026-10-19-guard-and-work-process-components.md`).

pub mod analysis;
pub mod assertions;
pub mod atomic;
pub mod backtest;
pub mod civil;
pub mod compliance;
pub mod corpus;
pub mod decision;
pub mod doctor;
#[cfg(unix)]
pub mod excerpt;
pub mod git;
pub mod gitconfig;
pub mod graduate;
pub mod guidance;
pub mod journal;
pub mod json;
pub mod land;
pub mod lsp;
pub mod mine;
pub mod payload;
pub mod publish_cmd;
pub mod push_record;
pub mod push_target;
#[cfg(unix)]
pub mod quiet;
pub mod rules;
pub mod session_state;
pub mod settings;
pub mod sha256;
pub mod shape;
pub mod shell;
pub mod shim;
pub mod skill_window;
pub mod stale;
pub mod stance;
pub mod term;
pub mod transcript;
pub mod ui;
