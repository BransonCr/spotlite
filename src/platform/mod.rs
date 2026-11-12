//! What differs between operating systems, in one place. The rest of spotlite calls these and
//! does not know which system it runs on. Linux and other Unix systems: `unix.rs`; Windows:
//! `windows.rs`. Code that runs a program or touches the system belongs here or in `engine`.

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;
