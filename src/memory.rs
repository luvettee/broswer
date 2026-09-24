#[cfg(target_os = "macos")]
pub fn resident_memory() -> Option<u64> {
    let mut usage: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
    let result = unsafe {
        libc::proc_pid_rusage(
            libc::getpid(),
            libc::RUSAGE_INFO_V4,
            (&mut usage as *mut libc::rusage_info_v4).cast(),
        )
    };
    (result == 0).then_some(usage.ri_resident_size)
}

#[cfg(not(target_os = "macos"))]
pub fn resident_memory() -> Option<u64> {
    None
}
