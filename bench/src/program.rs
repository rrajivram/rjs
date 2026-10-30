//! Benchmark programs: JavaScript templates expanded to a chosen size.
//!
//! A template is an ordinary `.js` file with one block marked for
//! repetition:
//!
//! ```js
//! // @description: What this measures (shown in the report).
//! let x = 1;               // runs once, before the repeated part
//! // @repeat
//! x = (x * 31 + {i}) % 7;  // repeated N times; {i} becomes 0, 1, 2, ...
//! // @end
//! console.log(x);          // runs once, after
//! ```
//!
//! A program without the markers is fixed: it runs as written, once per
//! engine, whatever sizes are requested.
//!
//! Programs should print a result at the end: the harness compares the
//! output of every engine, and printing keeps a JIT from discarding the work.
//! A fixed program with a `<name>.expected` file next to it is also a test:
//! its rjs output must match that file (see `bench/tests/expected_output.rs`).
//! Generate the file with Node: `node <name>.js > <name>.expected`.

use std::{fs, io, path::Path};

pub struct Template {
    pub name: String,
    pub description: String,
    prelude: String,
    /// The repeated block and what follows it; `None` for a fixed program.
    repeat: Option<(String, String)>,
}

impl Template {
    pub fn parse(name: &str, text: &str) -> Result<Self, String> {
        let lines: Vec<&str> = text.lines().collect();
        let marker = |m: &str| lines.iter().position(|l| l.trim() == m);
        let description = lines
            .iter()
            .find_map(|l| l.trim().strip_prefix("// @description:"))
            .unwrap_or("")
            .trim()
            .to_string();
        let join = |part: &[&str]| part.iter().map(|l| format!("{l}\n")).collect::<String>();
        let (prelude, repeat) = match (marker("// @repeat"), marker("// @end")) {
            (None, None) => (text.to_string(), None),
            (Some(start), Some(end)) if start < end => (
                join(&lines[..start]),
                Some((join(&lines[start + 1..end]), join(&lines[end + 1..]))),
            ),
            (Some(_), Some(_)) => {
                return Err(format!("{name}: `// @end` comes before `// @repeat`"));
            }
            _ => return Err(format!("{name}: needs both `// @repeat` and `// @end`, or neither")),
        };
        Ok(Self {
            name: name.to_string(),
            description,
            prelude,
            repeat,
        })
    }

    /// A program without a repeated block, which runs at one size only.
    pub fn is_fixed(&self) -> bool {
        self.repeat.is_none()
    }

    /// The sizes to run: the requested ones, or a single run for a fixed
    /// program.
    pub fn sizes(&self, requested: &[usize]) -> Vec<Option<usize>> {
        if self.is_fixed() {
            vec![None]
        } else {
            requested.iter().copied().map(Some).collect()
        }
    }

    /// Where to write the program for a given size.
    pub fn file_name(&self, size: Option<usize>) -> String {
        match size {
            Some(n) => format!("{}-{n}.js", self.name),
            None => format!("{}.js", self.name),
        }
    }

    /// The program with the repeated block written out `n` times. A fixed
    /// program is returned as written.
    pub fn expand(&self, n: Option<usize>) -> String {
        let Some((body, epilogue)) = &self.repeat else {
            return self.prelude.clone();
        };
        let n = n.unwrap_or(0);
        let mut out = String::with_capacity(self.prelude.len() + body.len() * n);
        out.push_str(&self.prelude);
        for i in 0..n {
            out.push_str(&body.replace("{i}", &i.to_string()));
        }
        out.push_str(epilogue);
        out
    }
}

/// Every `*.js` template in `dir`, sorted by name.
pub fn load_all(dir: &Path) -> io::Result<Vec<Template>> {
    let mut templates = vec![];
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "js") {
            let name = path.file_stem().unwrap_or_default().to_string_lossy();
            let text = fs::read_to_string(&path)?;
            templates.push(Template::parse(&name, &text).map_err(io::Error::other)?);
        }
    }
    templates.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(templates)
}

#[cfg(test)]
mod tests {
    use super::Template;

    #[test]
    fn expands_the_repeated_block() {
        let t = Template::parse(
            "t",
            "// @description: d\nlet x = 0;\n// @repeat\nx = x + {i};\n// @end\nconsole.log(x);\n",
        )
        .unwrap();
        assert_eq!(t.description, "d");
        assert!(!t.is_fixed());
        assert_eq!(
            t.expand(Some(3)),
            "// @description: d\nlet x = 0;\nx = x + 0;\nx = x + 1;\nx = x + 2;\nconsole.log(x);\n"
        );
    }

    #[test]
    fn rejects_mismatched_markers() {
        assert!(Template::parse("t", "let x = 0;\n// @end\n").is_err());
        assert!(Template::parse("t", "// @end\n// @repeat\n").is_err());
    }

    #[test]
    fn a_program_without_markers_is_fixed() {
        let t = Template::parse("t", "let x = 1;\nconsole.log(x);\n").unwrap();
        assert!(t.is_fixed());
        assert_eq!(t.sizes(&[10, 20]), vec![None]);
        assert_eq!(t.expand(None), "let x = 1;\nconsole.log(x);\n");
        assert_eq!(t.file_name(None), "t.js");
    }
}
