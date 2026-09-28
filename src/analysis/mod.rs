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
