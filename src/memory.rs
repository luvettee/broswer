//! Memory accounting and relief: what the app and its WebKit helpers use,
//! handing freed heap back to the system, and reacting to memory pressure.

use crate::msg::{Msg, MsgSender};

/// Memory as Activity Monitor counts it (physical footprint).
#[derive(Clone, Copy, Default)]
pub struct Usage {
    /// This app process.
    pub app: u64,
    /// WebKit web content, networking, and GPU processes working for this app.
    pub helpers: u64,
}

impl Usage {
    pub fn total(&self) -> u64 {
        self.app + self.helpers
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::c_void;
    use std::sync::OnceLock;

    use super::{Msg, MsgSender, Usage};

    #[repr(C)]
    pub struct DispatchSourceType {
        _private: [u8; 0],
    }

    const DISPATCH_MEMORYPRESSURE_WARN: usize = 0x2;
    const DISPATCH_MEMORYPRESSURE_CRITICAL: usize = 0x4;

    unsafe extern "C" {
        static _dispatch_source_type_memorypressure: DispatchSourceType;
        static _dispatch_main_q: c_void;
        fn dispatch_source_create(
            kind: *const DispatchSourceType,
            handle: usize,
            mask: usize,
            queue: *const c_void,
        ) -> *mut c_void;
        fn dispatch_set_context(object: *mut c_void, context: *mut c_void);
        fn dispatch_source_set_event_handler_f(source: *mut c_void, handler: extern "C" fn(*mut c_void));
        fn dispatch_source_get_data(source: *mut c_void) -> usize;
        fn dispatch_resume(object: *mut c_void);
        fn malloc_zone_pressure_relief(zone: *mut c_void, goal: usize) -> usize;
    }

    pub fn footprint(pid: libc::pid_t) -> Option<u64> {
        let mut usage: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
        let result = unsafe {
            libc::proc_pid_rusage(
                pid,
                libc::RUSAGE_INFO_V4,
                (&mut usage as *mut libc::rusage_info_v4).cast(),
            )
        };
        (result == 0).then_some(usage.ri_phys_footprint)
    }

    type Responsible = unsafe extern "C" fn(libc::pid_t) -> libc::pid_t;

    /// WebKit's helper processes are launched by launchd, not as our children;
    /// the system records this app as "responsible" for them. The lookup is
    /// not public API, so it is resolved at run time and skipped if missing.
    fn responsible() -> Option<Responsible> {
        static FUNCTION: OnceLock<Option<usize>> = OnceLock::new();
        let address = *FUNCTION.get_or_init(|| {
            let address = unsafe {
                libc::dlsym(libc::RTLD_DEFAULT, c"responsibility_get_pid_responsible_for_pid".as_ptr())
            };
            (!address.is_null()).then_some(address as usize)
        });
        address.map(|a| unsafe { std::mem::transmute::<usize, Responsible>(a) })
    }

    fn is_webkit_helper(pid: libc::pid_t) -> bool {
        let mut path = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        let len = unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32) };
        len > 0 && path[..len as usize].windows(17).any(|w| w == b"/com.apple.WebKit")
    }

    fn helper_footprint(own: libc::pid_t) -> u64 {
        let Some(responsible) = responsible() else {
            return 0;
        };
        let mut pids = vec![0 as libc::pid_t; 4096];
        let bytes = (pids.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
        let count = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
        if count <= 0 {
            return 0;
        }
        pids.truncate(count as usize);
        // Started from a terminal, the terminal is responsible for the app and
        // its helpers alike, so match on that and on the helper's binary.
        let owner = unsafe { responsible(own) };
        pids.into_iter()
            .filter(|&pid| pid != own && pid > 0 && unsafe { responsible(pid) } == owner)
            .filter(|&pid| is_webkit_helper(pid))
            .filter_map(footprint)
            .sum()
    }

    pub fn usage() -> Option<Usage> {
        let own = unsafe { libc::getpid() };
        let app = footprint(own)?;
        Some(Usage { app, helpers: helper_footprint(own) })
    }

    pub fn trim() {
        unsafe { malloc_zone_pressure_relief(std::ptr::null_mut(), 0) };
    }

    struct Watch {
        source: *mut c_void,
        tx: MsgSender,
    }

    extern "C" fn pressure(context: *mut c_void) {
        let watch = unsafe { &*(context as *const Watch) };
        let level = unsafe { dispatch_source_get_data(watch.source) };
        if level & (DISPATCH_MEMORYPRESSURE_WARN | DISPATCH_MEMORYPRESSURE_CRITICAL) != 0 {
            let critical = level & DISPATCH_MEMORYPRESSURE_CRITICAL != 0;
            let _ = watch.tx.send(Msg::MemoryPressure { critical });
        }
    }

    pub fn watch_pressure(tx: MsgSender) {
        unsafe {
            let source = dispatch_source_create(
                &_dispatch_source_type_memorypressure,
                0,
                DISPATCH_MEMORYPRESSURE_WARN | DISPATCH_MEMORYPRESSURE_CRITICAL,
                &_dispatch_main_q,
            );
            if source.is_null() {
                return;
            }
            // Lives for the whole run, like the source itself.
            let watch = Box::into_raw(Box::new(Watch { source, tx }));
            dispatch_set_context(source, watch.cast());
            dispatch_source_set_event_handler_f(source, pressure);
            dispatch_resume(source);
        }
    }
}

#[cfg(target_os = "macos")]
pub use mac::{trim, usage, watch_pressure};

#[cfg(not(target_os = "macos"))]
pub fn usage() -> Option<Usage> {
    None
}

#[cfg(not(target_os = "macos"))]
pub fn trim() {}

#[cfg(not(target_os = "macos"))]
pub fn footprint(_pid: i32) -> Option<u64> {
    None
}

#[cfg(not(target_os = "macos"))]
pub fn watch_pressure(_tx: MsgSender) {}
