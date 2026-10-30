//! `rjs [--verbose] <file.js>`: run a JavaScript file on the VM. Only what
//! the program itself logs is printed; errors go to stderr with a non-zero
//! exit code. `--verbose` also prints the parsed program, compiler tracing and
//! per-instruction statistics (which slow execution down noticeably).

use std::{env, fs, process::ExitCode};

use vm::executor::engine::JSEngine;

fn main() -> ExitCode {
    let mut verbose = false;
    let mut path = None;
    for arg in env::args().skip(1) {
        match arg.as_str() {
            "-v" | "--verbose" => verbose = true,
            _ if path.is_none() => path = Some(arg),
            _ => path = None,
        }
    }
    let Some(path) = path else {
        eprintln!("usage: rjs [--verbose] <file.js>");
        return ExitCode::from(2);
    };
    let src = match fs::read_to_string(&path) {
        Ok(src) => src,
        Err(e) => {
            eprintln!("rjs: {path}: {e}");
            return ExitCode::from(2);
        }
    };
    let mut engine = JSEngine::new();
    engine.verbose = verbose;
    match engine.exec(&src) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rjs: {e}");
            ExitCode::FAILURE
        }
    }
}
