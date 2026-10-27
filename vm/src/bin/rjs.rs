//! `rjs <file.js>`: run a JavaScript file on the VM. Only what the program
//! itself logs is printed; errors go to stderr with a non-zero exit code.

use std::{env, fs, process::ExitCode};

use vm::executor::engine::JSEngine;

fn main() -> ExitCode {
    let Some(path) = env::args().nth(1) else {
        eprintln!("usage: rjs <file.js>");
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
    engine.verbose = false;
    match engine.exec(&src) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rjs: {e}");
            ExitCode::FAILURE
        }
    }
}
