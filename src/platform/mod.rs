//! Platform back-ends: screen capture, window enumeration and OS glue.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::*;

#[cfg(not(windows))]
mod unix;
#[cfg(not(windows))]
pub use self::unix::*;

pub mod autostart;
