//! `rjs_phases <file.js>`: run a program on rjs one phase at a time (parse,
//! compile, execute) and report per-phase cost on stderr as one line:
//!
//! ```text
//! @@phases parse_ms=1.2 compile_ms=3.4 exec_ms=5.6 exec_rss=0 exec_heap=4096 code=65536
//! ```
//!
//! - `exec_rss`: growth of peak resident memory while executing, in bytes.
//! - `exec_heap`: the most heap held during execution beyond what was live
//!   when it started (exact, from a counting allocator).
//! - `code`: heap held by the compiled instructions.
//!
//! The program's own output goes to stdout.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    env, fs,
    process::ExitCode,
    sync::atomic::{AtomicUsize, Ordering::Relaxed},
    time::Instant,
};

use vm::executor::engine::JSEngine;

/// Counts live heap bytes, and the most that were live since `PEAK` was last
/// reset.
struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn grew(bytes: usize) {
    let live = LIVE.fetch_add(bytes, Relaxed) + bytes;
    PEAK.fetch_max(live, Relaxed);
}

// SAFETY: every method forwards to `System` unchanged and only adds counting.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded with the caller's guarantees.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            grew(layout.size());
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded with the caller's guarantees.
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            grew(layout.size());
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: forwarded with the caller's guarantees.
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size(), Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: forwarded with the caller's guarantees.
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            if new_size >= layout.size() {
                grew(new_size - layout.size());
            } else {
                LIVE.fetch_sub(layout.size() - new_size, Relaxed);
            }
        }
        p
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Peak resident set size of this process so far, in bytes.
fn peak_rss() -> u64 {
    // SAFETY: `rusage` is plain old data and getrusage fills it in.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: the pointer is valid for the call.
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    let raw = usage.ru_maxrss as u64;
    // Bytes on macOS, kilobytes on Linux.
    if cfg!(target_os = "macos") { raw } else { raw * 1024 }
}

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

fn main() -> ExitCode {
    let Some(path) = env::args().nth(1) else {
        eprintln!("usage: rjs_phases <file.js>");
        return ExitCode::from(2);
    };
    let src = match fs::read_to_string(&path) {
        Ok(src) => src,
        Err(e) => {
            eprintln!("rjs_phases: {path}: {e}");
            return ExitCode::from(2);
        }
    };
    let mut engine = JSEngine::new();
    engine.verbose = false;
    let result = (|| {
        let start = Instant::now();
        let program = engine.parse(&src)?;
        let parse_ms = ms(start);

        let before_compile = LIVE.load(Relaxed);
        let start = Instant::now();
        let code = engine.compile(&program)?;
        let compile_ms = ms(start);
        let code_bytes = LIVE.load(Relaxed).saturating_sub(before_compile);

        // Compiled code is meant to be run without its source, so the AST
        // isn't kept around during execution.
        drop(program);
        let heap_at_start = LIVE.load(Relaxed);
        PEAK.store(heap_at_start, Relaxed);
        let rss_at_start = peak_rss();
        let start = Instant::now();
        engine.run(&code)?;
        let exec_ms = ms(start);
        let exec_heap = PEAK.load(Relaxed) - heap_at_start;
        let exec_rss = peak_rss().saturating_sub(rss_at_start);

        eprintln!(
            "@@phases parse_ms={parse_ms} compile_ms={compile_ms} exec_ms={exec_ms} \
             exec_rss={exec_rss} exec_heap={exec_heap} code={code_bytes}"
        );
        Ok::<_, vm::error::JSError>(())
    })();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rjs_phases: {e}");
            ExitCode::FAILURE
        }
    }
}
