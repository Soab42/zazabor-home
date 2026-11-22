//! Keeping the AI work polite: its own thread pool (at most 6 threads, nice 10), and a pause while
//! the PC is busy with something else.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

/// The most threads the AI work may use.
pub const MAX_THREADS: usize = 6;

/// Lowers the calling thread's priority (nice 10).
pub fn lower_priority() {
    #[cfg(target_os = "linux")]
    // SAFETY: setpriority on this thread's own id; no memory is touched.
    unsafe {
        let tid = libc::syscall(libc::SYS_gettid) as libc::id_t;
        libc::setpriority(libc::PRIO_PROCESS, tid, 10);
    }
}

/// The pool every AI job runs inside (`pool().install(|| …)`): candle's matrix products use the
/// pool they're called from, so this caps their threads too.
pub fn pool() -> &'static rayon::ThreadPool {
    static P: OnceLock<rayon::ThreadPool> = OnceLock::new();
    P.get_or_init(|| {
        let n = std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, MAX_THREADS);
        rayon::ThreadPoolBuilder::new().num_threads(n).thread_name(|i| format!("ai-{i}")).start_handler(|_| lower_priority()).build().expect("a thread pool")
    })
}

/// The 1-minute load average.
fn load_now() -> f64 {
    std::fs::read_to_string("/proc/loadavg").ok().and_then(|s| s.split_whitespace().next().and_then(|v| v.parse().ok())).unwrap_or(0.0)
}

/// Above this the PC counts as busy (ZAZABOR_AI_LOAD_LIMIT overrides it).
fn load_limit() -> f64 {
    std::env::var("ZAZABOR_AI_LOAD_LIMIT").ok().and_then(|v| v.parse().ok()).unwrap_or(11.0)
}

/// Waits while the PC is busy (load average over the limit), calling `paused` once when it starts
/// waiting. Returns Err("Cancelled") if cancelled meanwhile.
pub fn wait_while_busy(cancel: &AtomicBool, paused: &dyn Fn()) -> Result<(), String> {
    let limit = load_limit();
    let mut announced = false;
    // The average is smoothed over a minute, so resume a little below the limit.
    while load_now() > if announced { limit - 2.0 } else { limit } {
        if cancel.load(Ordering::Relaxed) {
            return Err("Cancelled".into());
        }
        if !announced {
            announced = true;
            paused();
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("Cancelled".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pool_is_capped_and_runs_work() {
        let n = pool().install(rayon::current_num_threads);
        assert!((1..=MAX_THREADS).contains(&n));
    }

    #[test]
    fn cancelling_ends_the_wait() {
        // A limit nothing meets makes the PC "busy"; a cancelled job leaves at once.
        std::env::set_var("ZAZABOR_AI_LOAD_LIMIT", "-1");
        let cancel = AtomicBool::new(true);
        assert_eq!(wait_while_busy(&cancel, &|| {}).unwrap_err(), "Cancelled");
        std::env::remove_var("ZAZABOR_AI_LOAD_LIMIT");
    }
}
