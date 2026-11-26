//! Library for talking to the TOPPING E2x2 over its HID interface (and
//! recognising, behind `--experimental`, the E4x4 Pre and E8x8 Pre).

pub mod capture;
pub mod cli;
pub mod conn;
pub mod device;
pub mod error;
pub mod frame;
pub mod model;
pub mod official_app;
pub mod open;
pub mod rate;
pub mod state;
pub mod sync;
pub mod tpwork;
pub mod tui;
pub mod value;
pub mod workspace;
