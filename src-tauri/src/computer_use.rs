//! Computer use: an agent sees the apps open on this machine and, if the user allows it,
//! operates them. Ported from ViewCode. The app is the only gate: it decides every call
//! (setting, running turn, protected apps, approval, stale targets) and only then reaches
//! the native helper (`native/computer-driver`), which asks again before it prepares a
//! target and immediately before input.
//!
//! This is the gate's rules with no I/O: `policy` (modes, approvals, protected apps, the
//! decisions and their refusals), `targets` (the per-thread window, ref and shot tables
//! and what goes stale), `command` (recognising a plain launcher call) and `protocol`
//! (the helper's JSON lines). See `docs/computer-use.md`.
// Nothing calls these yet: the service that owns the helper and the agent tools come next.
#![allow(dead_code)]

pub(crate) mod command;
pub(crate) mod policy;
pub(crate) mod protocol;
pub(crate) mod settings;
#[cfg(test)]
mod stream_tests;
pub(crate) mod targets;
#[cfg(test)]
mod tests;
