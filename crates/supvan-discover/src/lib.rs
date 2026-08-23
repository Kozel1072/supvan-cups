//! Device discovery and the model registry, shared by the printer application
//! and the CLI.
//!
//! The three transports each have their own scanner — [`usb`] over sysfs,
//! [`bt`] over BlueZ D-Bus, [`ble`] over an LE scan — and all three agree on
//! what counts as a Supvan printer by going through [`models`]. Keeping them
//! in one crate is what stops the CLI and the daemon disagreeing about which
//! devices exist.

pub mod ble;
pub mod bt;
pub mod models;
pub mod usb;
pub mod util;
