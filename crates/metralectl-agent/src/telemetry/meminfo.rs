// SPDX-License-Identifier: MIT OR Apache-2.0

//! Host memory, which on unified-memory hardware is the meaningful figure.
//!
//! A GB10's 119.7 GB "GPU memory" is the host's LPDDR5X: `nvidia-smi` reports
//! no framebuffer at all, so this is where the number comes from.

pub use metralectl_core::hardware::capture::MemInfo;
pub use metralectl_core::hardware::capture::parse_meminfo as parse;

/// Whether host memory can be read here.
#[cfg(not(windows))]
pub fn available() -> bool {
    std::path::Path::new("/proc/meminfo").exists()
}

/// Whether host memory can be read here.
///
/// Always: `GlobalMemoryStatusEx` is part of the OS, not an optional file.
#[cfg(windows)]
pub fn available() -> bool {
    true
}

/// Read host memory.
#[cfg(not(windows))]
pub fn read() -> MemInfo {
    std::fs::read_to_string("/proc/meminfo")
        .map(|s| parse(&s))
        .unwrap_or_default()
}

/// Read host memory.
///
/// `ullAvailPhys` is the counterpart of `MemAvailable`, not of `MemFree`: it
/// counts memory obtainable without paging, so a machine that has just loaded
/// a large model reads as busy rather than as full.
#[cfg(windows)]
pub fn read() -> MemInfo {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut st: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    // Required by the API: it dispatches on the struct size it was compiled
    // against, and a zero here makes the call fail rather than misbehave.
    st.dwLength = u32::try_from(std::mem::size_of::<MEMORYSTATUSEX>()).unwrap_or(0);
    // SAFETY: `st` is a correctly sized, zeroed MEMORYSTATUSEX with dwLength
    // set, which is the entire contract; it is read only after success.
    if unsafe { GlobalMemoryStatusEx(&raw mut st) } == 0 {
        return MemInfo::default();
    }
    MemInfo {
        total_bytes: Some(st.ullTotalPhys),
        used_bytes: Some(st.ullTotalPhys.saturating_sub(st.ullAvailPhys)),
    }
}
