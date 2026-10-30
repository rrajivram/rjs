//! `--phases` mode: parse, compile and execution measured separately.
//!
//! Each engine runs through a wrapper that times its own phases and prints
//! one `@@phases key=value ...` line on stderr: `rjs_phases` for rjs, and
//! `bench/phases.js` for Node. Node parses and compiles in a single step
//! (`new vm.Script`), so its parse time is reported as part of compile.
//!
//! Memory, per phase:
//! - exec peak: growth of the process's peak resident memory during
//!   execution, measured the same way for every engine. It can understate
//!   execution memory when execution reuses memory freed by earlier phases.
//! - exec heap (rjs only): the most heap held during execution beyond what
//!   was live when it started, counted exactly by an allocator wrapper.
//! - code: for rjs, the heap held by the compiled instructions; for Node, the
//!   size of V8's serialized code cache. Both are what you'd keep in order to
//!   run the program again without recompiling it.

use std::{collections::HashMap, fmt::Write as _, fs, path::Path};

use crate::{Engine, Options, check_outputs, mb, program::Template, size_label};

/// One engine's cost for one program, per phase.
#[derive(Clone, Copy)]
struct Phases {
    parse_ms: Option<f64>,
    compile_ms: f64,
    exec_ms: f64,
    exec_rss: u64,
    exec_heap: Option<u64>,
    code: u64,
}

impl Phases {
    /// Reads the `@@phases` line from a wrapper's stderr.
    fn parse_report(stderr: &str) -> Result<Self, String> {
        let line = stderr
            .lines()
            .find_map(|l| l.strip_prefix("@@phases "))
            .ok_or("no @@phases report on stderr")?;
        let fields: HashMap<&str, &str> = line
            .split_whitespace()
            .filter_map(|kv| kv.split_once('='))
            .collect();
        let get = |key: &str| -> Result<Option<f64>, String> {
            fields
                .get(key)
                .map(|v| v.parse::<f64>().map_err(|_| format!("bad {key} `{v}`")))
                .transpose()
        };
        let need = |key: &str| get(key)?.ok_or_else(|| format!("report has no {key}"));
        Ok(Self {
            parse_ms: get("parse_ms")?,
            compile_ms: need("compile_ms")?,
            exec_ms: need("exec_ms")?,
            exec_rss: need("exec_rss")? as u64,
            exec_heap: get("exec_heap")?.map(|v| v as u64),
            code: need("code")? as u64,
        })
    }

    /// The middle value of each field across runs.
    fn median(samples: &[Phases]) -> Self {
        fn mid(mut values: Vec<f64>) -> f64 {
            values.sort_by(f64::total_cmp);
            values[values.len() / 2]
        }
        let field = |f: fn(&Phases) -> f64| mid(samples.iter().map(f).collect());
        let optional = |f: fn(&Phases) -> Option<f64>| {
            let values: Vec<f64> = samples.iter().filter_map(f).collect();
            (!values.is_empty()).then(|| mid(values))
        };
        Self {
            parse_ms: optional(|s| s.parse_ms),
            compile_ms: field(|s| s.compile_ms),
            exec_ms: field(|s| s.exec_ms),
            exec_rss: field(|s| s.exec_rss as f64) as u64,
            exec_heap: optional(|s| s.exec_heap.map(|h| h as f64)).map(|h| h as u64),
            code: field(|s| s.code as f64) as u64,
        }
    }
}

/// One discarded warm-up run, then `runs` measured runs.
fn sample(engine: &Engine, file: &Path, runs: usize) -> Result<Phases, String> {
    let once = || -> Result<Phases, String> {
        let out = engine
            .command(file)
            .output()
            .map_err(|e| format!("can't start: {e}"))?;
        let stderr = String::from_utf8_lossy(&out.stderr);
        if !out.status.success() {
            let first = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
            return Err(format!("failed: {first}"));
        }
        Phases::parse_report(&stderr)
    };
    once()?;
    let samples = (0..runs).map(|_| once()).collect::<Result<Vec<_>, _>>()?;
    Ok(Phases::median(&samples))
}

struct Row {
    benchmark: String,
    /// `None` for a fixed program.
    size: Option<usize>,
    engine: &'static str,
    outcome: Result<Phases, String>,
    output: &'static str,
}

pub fn run(
    opts: &Options,
    engines: &[Engine],
    templates: &[Template],
    out_dir: &Path,
    node_version: &str,
) -> Result<(), String> {
    println!("node {node_version}; {} runs per case, median reported", opts.runs);
    println!("node parses and compiles in one step; its total is shown under compile.");
    println!("exec peak: growth of peak resident memory while executing (all engines).");
    println!("exec heap: most heap held while executing, counted exactly (rjs only).");
    println!("code: rjs compiled instructions in memory; node V8 code cache size.\n");
    println!(
        "{:<18} {:>8} {:<15} {:>9} {:>10} {:>9} {:>10} {:>10} {:>9}  output",
        "", "N", "engine", "parse ms", "compile ms", "exec ms", "exec peak", "exec heap", "code"
    );
    let mut rows = vec![];
    for template in templates {
        println!("{}: {}", template.name, template.description);
        for size in template.sizes(&opts.sizes) {
            let file = out_dir.join(template.file_name(size));
            fs::write(&file, template.expand(size)).map_err(|e| e.to_string())?;
            let checks = if opts.check {
                check_outputs(engines, &file)
            } else {
                vec![Ok("-"); engines.len()]
            };
            for (engine, check) in engines.iter().zip(checks) {
                let (outcome, output) = match check {
                    Err(e) => (Err(e), "error"),
                    Ok(output) => (sample(engine, &file, opts.runs), output),
                };
                let row = Row {
                    benchmark: template.name.clone(),
                    size,
                    engine: engine.label,
                    outcome,
                    output,
                };
                print_row(&row);
                rows.push(row);
            }
        }
        println!();
    }
    if let Some(path) = &opts.csv {
        fs::write(path, csv(&rows)).map_err(|e| format!("writing {}: {e}", path.display()))?;
        println!("wrote {}", path.display());
    }
    Ok(())
}

fn print_row(row: &Row) {
    let lead = format!("{:<18} {:>8} {:<15}", "", size_label(row.size), row.engine);
    match &row.outcome {
        Ok(p) => println!(
            "{lead} {:>9} {:>10.1} {:>9.2} {:>7.1} MB {:>7} {:>9}  {}",
            p.parse_ms.map_or("-".into(), |v| format!("{v:.1}")),
            p.compile_ms,
            p.exec_ms,
            mb(p.exec_rss),
            p.exec_heap.map_or("-".into(), |h| format!("{:.1} MB", mb(h))),
            size(p.code),
            row.output
        ),
        Err(e) => println!("{lead} {e}"),
    }
}

/// Bytes as KB or MB, whichever reads better.
fn size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", mb(bytes))
    } else {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    }
}

fn csv(rows: &[Row]) -> String {
    let mut out = String::from(
        "benchmark,n,engine,parse_ms,compile_ms,exec_ms,exec_peak_rss_bytes,exec_heap_bytes,code_bytes,output,error\n",
    );
    for row in rows {
        match &row.outcome {
            Ok(p) => writeln!(
                out,
                "{},{},{},{},{:.3},{:.3},{},{},{},{},",
                row.benchmark,
                row.size.map_or(String::new(), |n| n.to_string()),
                row.engine,
                p.parse_ms.map_or(String::new(), |v| format!("{v:.3}")),
                p.compile_ms,
                p.exec_ms,
                p.exec_rss,
                p.exec_heap.map_or(String::new(), |h| h.to_string()),
                p.code,
                row.output
            ),
            Err(e) => writeln!(
                out,
                "{},{},{},,,,,,,{},\"{}\"",
                row.benchmark,
                row.size.map_or(String::new(), |n| n.to_string()),
                row.engine,
                row.output,
                e.replace('"', "\"\"")
            ),
        }
        .expect("writing to a String can't fail");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::Phases;

    #[test]
    fn reads_a_report_line() {
        let p = Phases::parse_report(
            "program noise\n@@phases parse_ms=1.5 compile_ms=2 exec_ms=3 exec_rss=4096 exec_heap=10 code=99\n",
        )
        .unwrap();
        assert_eq!(p.parse_ms, Some(1.5));
        assert_eq!((p.compile_ms, p.exec_ms), (2.0, 3.0));
        assert_eq!((p.exec_rss, p.exec_heap, p.code), (4096, Some(10), 99));

        let node = Phases::parse_report("@@phases compile_ms=2 exec_ms=3 exec_rss=0 code=1").unwrap();
        assert_eq!((node.parse_ms, node.exec_heap), (None, None));
        assert!(Phases::parse_report("nothing here").is_err());
    }
}
