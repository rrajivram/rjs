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
//! The VM has no loops yet, so repeating code is how a benchmark scales.
//! Programs should print a result at the end: the harness compares the
//! output of every engine, and printing keeps a JIT from discarding the work.

use std::{fs, io, path::Path};

pub struct Template {
    pub name: String,
    pub description: String,
    prelude: String,
    body: String,
    epilogue: String,
}

impl Template {
    pub fn parse(name: &str, text: &str) -> Result<Self, String> {
        let lines: Vec<&str> = text.lines().collect();
        let marker = |m: &str| {
            lines
                .iter()
                .position(|l| l.trim() == m)
                .ok_or_else(|| format!("{name}: missing a `{m}` line"))
        };
        let (start, end) = (marker("// @repeat")?, marker("// @end")?);
        if end < start {
            return Err(format!("{name}: `// @end` comes before `// @repeat`"));
        }
        let description = lines
            .iter()
            .find_map(|l| l.trim().strip_prefix("// @description:"))
            .unwrap_or("")
            .trim()
            .to_string();
        let join = |part: &[&str]| part.iter().map(|l| format!("{l}\n")).collect::<String>();
        Ok(Self {
            name: name.to_string(),
            description,
            prelude: join(&lines[..start]),
            body: join(&lines[start + 1..end]),
            epilogue: join(&lines[end + 1..]),
        })
    }

    /// The program with the repeated block written out `n` times.
    pub fn expand(&self, n: usize) -> String {
        let mut out = String::with_capacity(self.prelude.len() + self.body.len() * n);
        out.push_str(&self.prelude);
        for i in 0..n {
            out.push_str(&self.body.replace("{i}", &i.to_string()));
        }
        out.push_str(&self.epilogue);
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
        assert_eq!(
            t.expand(3),
            "// @description: d\nlet x = 0;\nx = x + 0;\nx = x + 1;\nx = x + 2;\nconsole.log(x);\n"
        );
    }

    #[test]
    fn rejects_missing_markers() {
        assert!(Template::parse("t", "let x = 0;\n// @end\n").is_err());
        assert!(Template::parse("t", "// @end\n// @repeat\n").is_err());
    }
}
