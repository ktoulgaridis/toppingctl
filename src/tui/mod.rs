//! The terminal UI: [`app`] holds the state and handles input, [`ui`] draws
//! it, [`run`] owns the terminal and the event loop.

pub mod app;
pub mod kitty;
pub mod pixel;
pub mod run;
pub mod theme;
pub mod ui;
pub mod widgets;
