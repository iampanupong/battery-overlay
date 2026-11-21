//! Battery sources. Each provider's parsing/merging logic is platform-independent;
//! only the OS queries are Windows-only.

pub mod bluetooth;

#[cfg(windows)]
use crate::model::Device;

/// Reads every provider once. Cheap enough to run on the UI thread.
#[cfg(windows)]
pub fn poll_all() -> Vec<Device> {
    bluetooth::poll()
}
