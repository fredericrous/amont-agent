//! Shell analysis: what a command may and must do, and why we think so.
//!
//! Four layers, each with one job:
//!
//! | layer | module | job |
//! |---|---|---|
//! | frontend | [`frontend`] | source → crate-owned [`ir`], spans on every node |
//! | semantics | [`interp`], [`state`], [`domain`] | abstract interpretation: state, control flow, execution counts |
//! | command models | [`models`] | a network client's arguments → its explicit transfers |
//! | policy | `rules::request_fanout` | whether the result warrants advice |
//!
//! Everything here is pure: the dialect is an input, and nothing reads the
//! world. The contract — inputs, assumptions, the supported subset, and what
//! `Unknown` and `Incomplete` mean — is in `docs/analysis.md`.

pub mod domain;
pub mod effects;
pub mod frontend;
pub mod interp;
pub mod ir;
pub mod limits;
pub mod models;
pub mod state;

use crate::rules::Dialect;
use effects::Effects;
use frontend::ParseError;
pub use interp::{Incomplete, ASSUMPTIONS};

/// What the analysis established about one command.
#[derive(Debug, Clone)]
pub struct Analysis {
    pub effects: Effects,
    /// Set when the analysis stopped short — a resource limit, or text the
    /// frontend could not parse. Nothing after `span` is known.
    pub incomplete: Option<Incomplete>,
}

/// Analyse `src` as run by `dialect`.
pub fn analyze(src: &str, dialect: Dialect) -> Analysis {
    match frontend::parse(src) {
        Ok(cmd) => {
            let out = interp::run(&cmd, src, dialect);
            Analysis {
                effects: out.effects,
                incomplete: out.incomplete,
            }
        }
        Err(e) => {
            let (span, why) = match e {
                ParseError::Syntax { why, span } => (span, why),
                ParseError::Limit(x) => (0..src.len(), x.describe()),
            };
            Analysis {
                effects: Effects {
                    contributions: Vec::new(),
                    unknown: vec![(span.clone(), why)],
                },
                incomplete: Some(Incomplete { span, why }),
            }
        }
    }
}
