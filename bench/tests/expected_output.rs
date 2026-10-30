//! Runs every fixed program in `bench/programs` that has a `<name>.expected`
//! file on rjs, and checks that it prints exactly that.

use std::{fs, path::Path, process::Command};

#[test]
fn programs_print_their_expected_output() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("programs");
    let mut checked = 0;
    for entry in fs::read_dir(&dir).expect("bench/programs is readable") {
        let expected_file = entry.expect("directory entry").path();
        if expected_file.extension().is_none_or(|e| e != "expected") {
            continue;
        }
        let program = expected_file.with_extension("js");
        let expected = fs::read_to_string(&expected_file).expect("expected output is readable");
        let out = Command::new(env!("CARGO_BIN_EXE_rjs_phases"))
            .arg(&program)
            .output()
            .expect("rjs_phases runs");
        assert!(
            out.status.success(),
            "{}: {}",
            program.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            normalize(&String::from_utf8_lossy(&out.stdout)),
            normalize(&expected),
            "{}",
            program.display()
        );
        checked += 1;
    }
    assert!(checked > 0, "no .expected files in {}", dir.display());
}

/// Ignores trailing whitespace, and the VM's per-instruction statistics line.
fn normalize(output: &str) -> String {
    output
        .lines()
        .filter(|line| !line.starts_with("[Statistics {"))
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_string()
}
