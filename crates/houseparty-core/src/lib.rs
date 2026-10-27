//! Pure alarm-card logic: no web, no I/O, no dependencies.
//!
//! Everything security-relevant lives here so it can be tested with plain
//! `cargo test`. The card is *not* a security boundary (Home Assistant
//! validates codes and enforces policy). This crate's job is to never mislead
//! the user and never weaken what HA enforces.
#![deny(unsafe_code)]

pub mod alarm;
pub mod countdown;
pub mod keypad;
pub mod pin;
pub mod policy;
pub mod sensors;
