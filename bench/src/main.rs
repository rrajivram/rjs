//! Compares rjs with Node on wall time, CPU time and peak memory.
//!
//! ```text
//! cargo run -p bench --release -- [options]
//!
//!   --sizes 1000,5000,10000   how many times to repeat each program's body
//!   --runs 5                  measured runs per case (the median is reported)
//!   --filter arrays           only programs whose name contains this text
//!   --csv results.csv         also write every result to a CSV file
//!   --node node               the Node executable
//!   --rjs path/to/rjs         use this rjs binary instead of building one
//!   --no-check                skip comparing program output between engines
//!   --phases                  time parse, compile and execution separately
//! ```
//!
//! Every program runs as a fresh process. By default, time and memory are
//! for the whole process, reported both in total and above an empty program
//! on the same engine, which removes each runtime's fixed startup cost
//! (Node's is roughly 40MB). With `--phases`, each process measures its own
//! phases instead (see `phases.rs`). To add a benchmark, drop a template into
//! `bench/programs/` (see `program.rs`).

#[cfg(not(unix))]
compile_error!("the benchmark harness needs a Unix-like OS: it uses wait4 to read peak memory");

mod measure;
mod phases;
mod program;

use std::{
    env,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

use measure::{Sample, measure, median};

struct Options {
    sizes: Vec<usize>,
    runs: usize,
    filter: Option<String>,
    csv: Option<PathBuf>,
    node: String,
    rjs: Option<PathBuf>,
    check: bool,
    phases: bool,
}

impl Options {
    fn parse() -> Result<Self, String> {
        let mut opts = Options {
            sizes: vec![1_000, 5_000, 10_000],
            runs: 5,
            filter: None,
            csv: None,
            node: "node".into(),
            rjs: None,
            check: true,
            phases: false,
        };
        let mut args = env::args().skip(1);
        while let Some(flag) = args.next() {
            let mut value = || args.next().ok_or_else(|| format!("{flag} needs a value"));
            match flag.as_str() {
                "--sizes" => {
                    opts.sizes = value()?
                        .split(',')
                        .map(|s| s.trim().parse().map_err(|_| format!("bad size `{s}`")))
                        .collect::<Result<_, _>>()?;
                }
                "--runs" => {
                    opts.runs = value()?.parse().map_err(|_| "bad --runs".to_string())?;
                    if opts.runs == 0 {
                        return Err("--runs must be at least 1".into());
                    }
                }
                "--filter" => opts.filter = Some(value()?),
                "--csv" => opts.csv = Some(value()?.into()),
                "--node" => opts.node = value()?,
                "--rjs" => opts.rjs = Some(value()?.into()),
                "--no-check" => opts.check = false,
                "--phases" => opts.phases = true,
                "-h" | "--help" => return Err(String::new()),
                other => return Err(format!("unknown option `{other}`")),
            }
        }
        Ok(opts)
    }
}

/// A way of running a JavaScript file.
struct Engine {
    label: &'static str,
    program: PathBuf,
    args: Vec<String>,
}

impl Engine {
    fn command(&self, file: &Path) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args).arg(file);
        cmd
    }
}

/// The result for one program, size and engine.
struct Row {
    benchmark: String,
    size: usize,
    engine: &'static str,
    outcome: Result<Sample, String>,
    output: &'static str,
}

fn main() -> ExitCode {
    let opts = match Options::parse() {
        Ok(opts) => opts,
        Err(msg) => {
            if !msg.is_empty() {
                eprintln!("bench: {msg}\n");
            }
            eprintln!("{}", USAGE);
            return ExitCode::from(2);
        }
    };
    match run(&opts) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("bench: {msg}");
            ExitCode::FAILURE
        }
    }
}

const USAGE: &str = "usage: cargo run -p bench --release -- [--sizes 1000,10000] [--runs 5] \
[--filter name] [--csv out.csv] [--node node] [--rjs path] [--no-check] [--phases]";

fn run(opts: &Options) -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("bench lives inside the workspace")
        .to_path_buf();
    let node_version = version(&opts.node).ok_or_else(|| {
        format!(
            "can't run `{} --version`; pass --node with the path to Node",
            opts.node
        )
    })?;
    // In phase mode each engine runs a wrapper that times its own phases.
    let (rjs, node_args): (PathBuf, Vec<String>) = if opts.phases {
        let wrapper = root.join("bench/phases.js").to_string_lossy().into_owned();
        (cargo_build(&root, "bench", "rjs_phases")?, vec![wrapper])
    } else {
        let rjs = match &opts.rjs {
            Some(path) => path.clone(),
            None => cargo_build(&root, "vm", "rjs")?,
        };
        (rjs, vec![])
    };
    let engines = [
        Engine {
            label: "rjs",
            program: rjs,
            args: vec![],
        },
        Engine {
            label: "node",
            program: opts.node.clone().into(),
            args: node_args.clone(),
        },
        Engine {
            label: "node --jitless",
            program: opts.node.clone().into(),
            args: [vec!["--jitless".to_string()], node_args].concat(),
        },
    ];

    let mut templates = program::load_all(&root.join("bench/programs"))
        .map_err(|e| format!("loading bench/programs: {e}"))?;
    if let Some(filter) = &opts.filter {
        templates.retain(|t| t.name.contains(filter.as_str()));
    }
    if templates.is_empty() {
        return Err("no benchmark programs matched".into());
    }

    let out_dir = root.join("target/bench");
    fs::create_dir_all(&out_dir).map_err(|e| format!("creating {}: {e}", out_dir.display()))?;
    if opts.phases {
        return phases::run(opts, &engines, &templates, &out_dir, &node_version);
    }

    println!(
        "node {node_version}; {} runs per case, median reported",
        opts.runs
    );
    println!("\"above empty\" subtracts the same engine running an empty program.\n");

    let empty = out_dir.join("empty.js");
    fs::write(&empty, "").map_err(|e| e.to_string())?;
    let mut baselines = vec![];
    for engine in &engines {
        let baseline = sample(engine, &empty, opts.runs)
            .map_err(|e| format!("{} can't run an empty program: {e}", engine.label))?;
        println!(
            "empty program on {:<15} {:>8.1} ms {:>8.1} MB",
            engine.label,
            ms(baseline.wall),
            mb(baseline.peak_rss)
        );
        baselines.push(baseline);
    }

    println!();
    print_header();
    let mut rows = vec![];
    for template in &templates {
        println!("{}: {}", template.name, template.description);
        for &size in &opts.sizes {
            let file = out_dir.join(format!("{}-{size}.js", template.name));
            fs::write(&file, template.expand(size)).map_err(|e| e.to_string())?;
            let checks = if opts.check {
                check_outputs(&engines, &file)
            } else {
                vec![Ok("-"); engines.len()]
            };
            for ((engine, baseline), check) in engines.iter().zip(&baselines).zip(checks) {
                let row = match check {
                    Err(e) => Row {
                        benchmark: template.name.clone(),
                        size,
                        engine: engine.label,
                        outcome: Err(e),
                        output: "error",
                    },
                    Ok(output) => Row {
                        benchmark: template.name.clone(),
                        size,
                        engine: engine.label,
                        outcome: sample(engine, &file, opts.runs).map_err(|e| e.to_string()),
                        output,
                    },
                };
                print_row(&row, baseline);
                rows.push((row, *baseline));
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

/// One discarded warm-up run (it pays for cold file and disk caches), then
/// `runs` measured runs.
fn sample(engine: &Engine, file: &Path, runs: usize) -> std::io::Result<Sample> {
    measure(engine.command(file))?;
    let samples = (0..runs)
        .map(|_| measure(engine.command(file)))
        .collect::<std::io::Result<Vec<_>>>()?;
    Ok(median(&samples))
}

/// Runs the program once on every engine and compares output with plain
/// `node`, ignoring trailing whitespace. Each entry is `"ok"`/`"differs"`,
/// or the engine's error.
fn check_outputs(engines: &[Engine], file: &Path) -> Vec<Result<&'static str, String>> {
    let outputs: Vec<Result<String, String>> = engines
        .iter()
        .map(|engine| {
            let out = engine
                .command(file)
                .output()
                .map_err(|e| format!("can't start: {e}"))?;
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                let first = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
                return Err(format!("failed: {first}"));
            }
            Ok(normalize(&String::from_utf8_lossy(&out.stdout)))
        })
        .collect();
    let reference = outputs[1].clone().ok();
    outputs
        .into_iter()
        .map(|output| {
            let output = output?;
            Ok(match &reference {
                Some(expected) if *expected == output => "ok",
                Some(_) => "differs",
                None => "-",
            })
        })
        .collect()
}

fn normalize(output: &str) -> String {
    output
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_string()
}

/// Builds a release binary of the workspace and returns its path.
fn cargo_build(root: &Path, package: &str, bin: &str) -> Result<PathBuf, String> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(cargo)
        .args(["build", "--release", "--quiet", "-p", package, "--bin", bin])
        .current_dir(root)
        .status()
        .map_err(|e| format!("running cargo build: {e}"))?;
    if !status.success() {
        return Err(format!("building {bin} failed"));
    }
    Ok(root.join("target/release").join(bin))
}

fn version(program: &str) -> Option<String> {
    let out = Command::new(program).arg("--version").output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn ms(d: std::time::Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn print_header() {
    println!(
        "{:<18} {:>8} {:<15} {:>9} {:>11} {:>9} {:>9} {:>11}  output",
        "", "N", "engine", "time ms", "above empty", "cpu ms", "peak MB", "above empty"
    );
}

fn print_row(row: &Row, baseline: &Sample) {
    let lead = format!("{:<18} {:>8} {:<15}", "", row.size, row.engine);
    match &row.outcome {
        Ok(s) => println!(
            "{lead} {:>9.1} {:>11.1} {:>9.1} {:>9.1} {:>11.1}  {}",
            ms(s.wall),
            ms(s.wall.saturating_sub(baseline.wall)),
            ms(s.cpu),
            mb(s.peak_rss),
            mb(s.peak_rss.saturating_sub(baseline.peak_rss)),
            row.output
        ),
        Err(e) => println!("{lead} {e}"),
    }
}

fn csv(rows: &[(Row, Sample)]) -> String {
    let mut out = String::from(
        "benchmark,n,engine,wall_ms,wall_above_empty_ms,cpu_ms,peak_rss_bytes,peak_rss_above_empty_bytes,output,error\n",
    );
    for (row, baseline) in rows {
        match &row.outcome {
            Ok(s) => writeln!(
                out,
                "{},{},{},{:.3},{:.3},{:.3},{},{},{},",
                row.benchmark,
                row.size,
                row.engine,
                ms(s.wall),
                ms(s.wall.saturating_sub(baseline.wall)),
                ms(s.cpu),
                s.peak_rss,
                s.peak_rss.saturating_sub(baseline.peak_rss),
                row.output
            ),
            Err(e) => writeln!(
                out,
                "{},{},{},,,,,,{},\"{}\"",
                row.benchmark,
                row.size,
                row.engine,
                row.output,
                e.replace('"', "\"\"")
            ),
        }
        .expect("writing to a String can't fail");
    }
    out
}
