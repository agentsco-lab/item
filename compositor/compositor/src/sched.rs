//! The compositor's place on the CPUs, as a phone's render thread has it:
//! its loop (where every frame is drawn) runs round-robin at a low real-time
//! priority, ahead of the apps' threads, and on the big cores (the A76s and
//! the prime: 4-7 on the Snapdragon 855), which wake quicker and run a frame
//! in less time than the little ones.
//!
//! Threads it starts afterwards are back to the normal policy (reset on
//! fork); the processes it starts (apps, gnome-session) get every core back
//! (`unpin`, before they exec). The real-time priority needs the unit's
//! leave (`LimitRTPRIO`, tools/session-run.sh; item.service); without it
//! the loop keeps its normal priority and only the cores are chosen.
//! `NO_SCHED=1` leaves both alone.

/// The big cores.
const BIG: std::ops::RangeInclusive<usize> = 4..=7;
/// Round-robin, low: above every normal thread, below the kernel's own.
const PRIORITY: i32 = 5;

/// The loop's thread on the big cores, and real-time if it may be.
pub fn favour_this_thread() {
    if std::env::var_os("NO_SCHED").is_some() {
        return;
    }
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        for c in BIG.filter(|&c| c < cores) {
            libc::CPU_SET(c, &mut set);
        }
        if libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set) == 0 {
            tracing::info!("sched: the loop on cores {}-{}", BIG.start(), BIG.end().min(&(cores - 1)));
        } else {
            tracing::warn!("sched: cores: {}", std::io::Error::last_os_error());
        }
        let param = libc::sched_param { sched_priority: PRIORITY };
        if libc::sched_setscheduler(0, libc::SCHED_RR | libc::SCHED_RESET_ON_FORK, &param) == 0 {
            tracing::info!("sched: the loop round-robin at {PRIORITY}");
        } else {
            tracing::warn!("sched: no real-time priority ({}): normal", std::io::Error::last_os_error());
        }
    }
}

/// In a child before it execs: every core again.
pub fn unpin() {
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        for c in 0..libc::CPU_SETSIZE as usize {
            libc::CPU_SET(c, &mut set);
        }
        libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set);
    }
}
