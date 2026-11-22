//! The terminal UI: [`app`] holds the state and handles input, [`ui`] draws
//! it, [`run`] owns the terminal and the event loop.

pub mod app;
pub mod run;
pub mod theme;
pub mod ui;
