//! Running a program as a child process and measuring it.

use std::{
    io,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// One measured run.
#[derive(Debug, Clone, Copy)]
pub struct Sample {
    /// Elapsed time from spawning the process to its exit, including startup.
    pub wall: Duration,
    /// User plus system CPU time.
    pub cpu: Duration,
    /// Peak resident set size: the most physical memory the process held at
    /// any one time.
    pub peak_rss: u64,
}

/// Runs `cmd` to completion with all output discarded, and measures it.
/// Fails if the process can't start or exits unsuccessfully.
pub fn measure(mut cmd: Command) -> io::Result<Sample> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let start = Instant::now();
    let child = cmd.spawn()?;
    let (status, usage) = wait4(child.id())?;
    let wall = start.elapsed();
    if !(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0) {
        return Err(io::Error::other(format!(
            "exited unsuccessfully (wait status {status})"
        )));
    }
    Ok(Sample {
        wall,
        cpu: timeval(usage.ru_utime) + timeval(usage.ru_stime),
        peak_rss: peak_rss_bytes(&usage),
    })
}

/// Waits for the child and returns its wait status and resource usage. Unlike
/// `getrusage(RUSAGE_CHILDREN)`, which reports the maximum over every child
/// so far, this gives the usage of this one process.
fn wait4(pid: u32) -> io::Result<(i32, libc::rusage)> {
    let mut status = 0;
    // SAFETY: `rusage` is plain old data, so all-zero is a valid value; wait4
    // overwrites it.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    loop {
        // SAFETY: `pid` is our own child, not yet waited on, and both
        // pointers are valid for the duration of the call.
        let r = unsafe { libc::wait4(pid as libc::pid_t, &mut status, 0, &mut usage) };
        if r >= 0 {
            return Ok((status, usage));
        }
        let err = io::Error::last_os_error();
        if err.kind() != io::ErrorKind::Interrupted {
            return Err(err);
        }
    }
}

fn timeval(tv: libc::timeval) -> Duration {
    Duration::new(tv.tv_sec as u64, tv.tv_usec as u32 * 1000)
}

/// `ru_maxrss` is in bytes on macOS but kilobytes on Linux.
fn peak_rss_bytes(usage: &libc::rusage) -> u64 {
    let raw = usage.ru_maxrss as u64;
    if cfg!(target_os = "macos") {
        raw
    } else {
        raw * 1024
    }
}

/// The middle value of the samples, field by field.
pub fn median(samples: &[Sample]) -> Sample {
    fn mid<T: Ord + Copy>(mut values: Vec<T>) -> T {
        values.sort();
        values[values.len() / 2]
    }
    Sample {
        wall: mid(samples.iter().map(|s| s.wall).collect()),
        cpu: mid(samples.iter().map(|s| s.cpu).collect()),
        peak_rss: mid(samples.iter().map(|s| s.peak_rss).collect()),
    }
}
