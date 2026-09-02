//! Resident-set sampling for health reporting.
//!
//! # Unsafe boundary
//!
//! This module is on the workspace `unsafe_code` exception list: resident memory is only
//! exposed through platform C APIs. Every call is a read-only query with no pointers handed
//! back to the OS beyond the stack slot being filled, and each block documents its argument.
#![allow(unsafe_code)]

pub(crate) fn current_rss_bytes() -> Option<u64> {
    platform::current_rss_bytes()
}

pub(crate) fn process_cpu_time_ms() -> Option<u64> {
    platform::process_cpu_time_ms()
}

#[cfg(target_os = "linux")]
mod platform {
    pub(super) fn current_rss_bytes() -> Option<u64> {
        let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
        let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
        // SAFETY: `sysconf` is a pure lookup with no pointer arguments; a non-positive
        // result is treated as unavailable below.
        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if page_size <= 0 {
            return None;
        }
        Some(pages.saturating_mul(page_size.cast_unsigned()))
    }

    pub(super) fn process_cpu_time_ms() -> Option<u64> {
        let mut value = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: `clock_gettime` only fills the caller-provided stack slot; a non-zero
        // status means the process CPU clock is unavailable.
        let status = unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut value) };
        if status != 0 {
            return None;
        }
        let nanos = u128::from(value.tv_sec.unsigned_abs()) * 1_000_000_000
            + u128::from(value.tv_nsec.unsigned_abs());
        Some(u64::try_from(nanos / 1_000_000).unwrap_or(u64::MAX))
    }
}

#[cfg(target_os = "macos")]
mod platform {
    pub(super) fn current_rss_bytes() -> Option<u64> {
        use std::mem::MaybeUninit;

        let mut info = MaybeUninit::<libc::mach_task_basic_info>::uninit();
        let mut count = libc::MACH_TASK_BASIC_INFO_COUNT;
        // SAFETY: `count` is initialised to the element count that matches
        // `mach_task_basic_info`, and `info` provides writable storage of exactly that size,
        // so the kernel cannot write out of bounds.
        #[allow(deprecated)]
        let kr = unsafe {
            libc::task_info(
                libc::mach_task_self(),
                libc::MACH_TASK_BASIC_INFO,
                info.as_mut_ptr().cast(),
                &mut count,
            )
        };
        if kr != libc::KERN_SUCCESS {
            return None;
        }
        // SAFETY: `task_info` reported `KERN_SUCCESS` above, so it fully initialised `info`.
        let info = unsafe { info.assume_init() };
        Some(info.resident_size)
    }

    pub(super) fn process_cpu_time_ms() -> Option<u64> {
        let mut value = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: `clock_gettime` only fills the caller-provided stack slot; a non-zero
        // status means the process CPU clock is unavailable.
        let status = unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut value) };
        if status != 0 {
            return None;
        }
        #[allow(clippy::cast_sign_loss)]
        let nanos = (value.tv_sec as u128) * 1_000_000_000 + value.tv_nsec as u128;
        Some(u64::try_from(nanos / 1_000_000).unwrap_or(u64::MAX))
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use std::mem::size_of;

    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    use windows_sys::core::BOOL;

    type MemoryInfoQuery =
        unsafe extern "system" fn(HANDLE, *mut PROCESS_MEMORY_COUNTERS, u32) -> BOOL;

    pub(super) fn current_rss_bytes() -> Option<u64> {
        current_rss_bytes_with(GetProcessMemoryInfo)
    }

    fn current_rss_bytes_with(query: MemoryInfoQuery) -> Option<u64> {
        let mut counters = PROCESS_MEMORY_COUNTERS_EX {
            cb: u32::try_from(size_of::<PROCESS_MEMORY_COUNTERS_EX>()).ok()?,
            ..PROCESS_MEMORY_COUNTERS_EX::default()
        };
        let counters_ptr = std::ptr::addr_of_mut!(counters).cast::<PROCESS_MEMORY_COUNTERS>();
        let succeeded = unsafe { query(GetCurrentProcess(), counters_ptr, counters.cb) } != 0;
        if !succeeded {
            return None;
        }
        u64::try_from(counters.WorkingSetSize).ok()
    }

    pub(super) fn process_cpu_time_ms() -> Option<u64> {
        use windows_sys::Win32::Foundation::FILETIME;
        use windows_sys::Win32::System::Threading::GetProcessTimes;

        let mut creation = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        // SAFETY: each pointer is a writable stack slot for exactly the five FILETIMEs
        // `GetProcessTimes` fills; the pseudo process handle needs no closing.
        let succeeded = unsafe {
            GetProcessTimes(
                GetCurrentProcess(),
                &mut creation,
                &mut exit,
                &mut kernel,
                &mut user,
            )
        } != 0;
        if !succeeded {
            return None;
        }
        let ticks =
            |value: FILETIME| ((value.dwHighDateTime as u128) << 32) | value.dwLowDateTime as u128;
        let nanos = (ticks(kernel) + ticks(user)) * 100;
        Some(u64::try_from(nanos / 1_000_000).unwrap_or(u64::MAX))
    }

    #[cfg(test)]
    mod tests {
        use std::hint::black_box;
        use std::process::Command;
        use std::time::Instant;

        use windows_sys::Win32::System::Threading::GetProcessHandleCount;

        use super::*;

        const SAMPLE_COUNT: usize = 10_000;
        const CHILD_ENV: &str = "MUTSUKI_WINDOWS_RSS_STABILITY_CHILD";
        const TEST_NAME: &str = "process_metrics::platform::tests::windows_rss_collection_10000_samples_has_no_handle_growth";

        #[test]
        fn windows_rss_collection_returns_working_set() {
            assert!(current_rss_bytes().unwrap_or(0) > 0);
        }

        #[test]
        fn windows_rss_collection_failure_returns_none() {
            unsafe extern "system" fn fail_query(
                _process: HANDLE,
                _counters: *mut PROCESS_MEMORY_COUNTERS,
                _size: u32,
            ) -> BOOL {
                0
            }

            assert_eq!(current_rss_bytes_with(fail_query), None);
        }

        #[test]
        fn windows_rss_collection_10000_samples_has_no_handle_growth() {
            if std::env::var_os(CHILD_ENV).is_none() {
                let output = Command::new(std::env::current_exe().expect("current test binary"))
                    .args([TEST_NAME, "--exact", "--nocapture"])
                    .env(CHILD_ENV, "1")
                    .output()
                    .expect("run isolated RSS stability test");
                eprint!("{}", String::from_utf8_lossy(&output.stderr));
                assert!(
                    output.status.success(),
                    "isolated RSS stability test failed:\n{}",
                    String::from_utf8_lossy(&output.stdout)
                );
                return;
            }

            black_box(current_rss_bytes().expect("warm up Windows RSS collection"));
            let handles_before = current_process_handle_count();
            let started = Instant::now();
            for _ in 0..SAMPLE_COUNT {
                black_box(current_rss_bytes().expect("collect Windows working set"));
            }
            let elapsed = started.elapsed();
            let handles_after = current_process_handle_count();

            assert_eq!(
                handles_after, handles_before,
                "RSS collection must not create or leak process handles"
            );
            eprintln!(
                "windows_rss_samples={} elapsed_ns={} ns_per_sample={} handles_before={} handles_after={}",
                SAMPLE_COUNT,
                elapsed.as_nanos(),
                elapsed.as_nanos() / SAMPLE_COUNT as u128,
                handles_before,
                handles_after
            );
        }

        fn current_process_handle_count() -> u32 {
            let mut count = 0;
            let succeeded = unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut count) } != 0;
            assert!(succeeded, "GetProcessHandleCount failed");
            count
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod platform {
    pub(super) fn current_rss_bytes() -> Option<u64> {
        None
    }
}
