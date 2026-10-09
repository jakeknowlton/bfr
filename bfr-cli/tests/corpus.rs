//! Runs every `.bfr` file under `corpus/` through the `bfr` binary and
//! compares the printed IR to the file's expected block.
//!
//! A corpus file has three parts, separated by blank lines: a comment
//! saying what it checks, a `# bfr:` line giving the command-line
//! arguments followed by the input IR, and a `# expect:` line followed by
//! the expected output as comment lines. The input is written exactly as
//! the printer would print it, with no comments of its own. Setting
//! `BFR_UPDATE=1` rewrites the input and the expected block to match
//! instead of failing.

use std::fmt::Write as _;
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::Command;

const CORPUS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../corpus");

/// A corpus file taken apart. The ranges are line numbers into the file.
struct Case {
    args: Vec<String>,
    input: Range<usize>,
    expect: Range<usize>,
}

fn find_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|entry| entry.expect("readable entry").path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            find_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "bfr") {
            out.push(path);
        }
    }
}

fn parse_case(lines: &[&str]) -> Result<Case, String> {
    match lines.first() {
        Some(first) if first.starts_with("# ") && !first.starts_with("# bfr:") => {}
        _ => return Err("the first line must be a comment saying what the file checks".into()),
    }
    let mut args = None;
    let mut bfr_at = None;
    let mut expect_at = None;
    for (i, line) in lines.iter().enumerate() {
        if let Some(rest) = line.strip_prefix("# bfr:") {
            if bfr_at.replace(i).is_some() {
                return Err("more than one `# bfr:` line".into());
            }
            args = Some(rest.split_whitespace().map(String::from).collect());
        } else if line.trim_end() == "# expect:" && expect_at.replace(i).is_some() {
            return Err("more than one `# expect:` line".into());
        }
    }
    let (Some(args), Some(bfr_at)) = (args, bfr_at) else {
        return Err("no `# bfr:` line".into());
    };
    let expect_at = expect_at.ok_or("no `# expect:` line")?;
    if expect_at < bfr_at {
        return Err("`# expect:` comes before `# bfr:`".into());
    }

    // The input is everything between the two directives, less the blank
    // lines around it.
    let mut input = bfr_at + 1..expect_at;
    while input.start < input.end && lines[input.start].trim().is_empty() {
        input.start += 1;
    }
    while input.start < input.end && lines[input.end - 1].trim().is_empty() {
        input.end -= 1;
    }

    let mut expect_end = expect_at + 1;
    while expect_end < lines.len() && lines[expect_end].starts_with('#') {
        expect_end += 1;
    }
    Ok(Case {
        args,
        input,
        expect: expect_at + 1..expect_end,
    })
}

fn join(lines: &[&str]) -> String {
    lines.iter().map(|line| format!("{line}\n")).collect()
}

/// The expected block with its comment markers removed.
fn uncomment(lines: &[&str]) -> String {
    lines
        .iter()
        .map(|line| {
            let body = &line[1..];
            format!("{}\n", body.strip_prefix(' ').unwrap_or(body))
        })
        .collect()
}

fn comment(text: &str) -> String {
    text.lines()
        .map(|line| {
            if line.is_empty() {
                "#\n".to_string()
            } else {
                format!("# {line}\n")
            }
        })
        .collect()
}

/// `lines` with each range replaced by its text. The ranges are in order
/// and do not overlap.
fn splice(lines: &[&str], edits: &[(Range<usize>, String)]) -> String {
    let mut out = String::new();
    let mut next = 0;
    for (range, replacement) in edits {
        out.push_str(&join(&lines[next..range.start]));
        out.push_str(replacement);
        next = range.end;
    }
    out.push_str(&join(&lines[next..]));
    out
}

#[test]
fn corpus() {
    let mut files = Vec::new();
    find_files(Path::new(CORPUS), &mut files);
    assert!(!files.is_empty(), "no .bfr files under {CORPUS}");
    let updating = std::env::var_os("BFR_UPDATE").is_some_and(|v| !v.is_empty());

    let mut failures = String::new();
    for path in &files {
        let name = path.strip_prefix(CORPUS).unwrap_or(path).display();
        let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("{name}: {e}"));
        let lines: Vec<&str> = text.lines().collect();
        let case = match parse_case(&lines) {
            Ok(case) => case,
            Err(e) => {
                let _ = writeln!(failures, "{name}: {e}\n");
                continue;
            }
        };

        let input = join(&lines[case.input.clone()]);
        let printed = match bfr::ir::print::parse(&input) {
            Ok(program) => bfr::ir::print::print(&program),
            Err((offset, message)) => {
                let _ = writeln!(failures, "{name}: input does not parse: byte {offset}: {message}\n");
                continue;
            }
        };
        let output = Command::new(env!("CARGO_BIN_EXE_bfr"))
            .args(&case.args)
            .arg("--emit")
            .arg("ir")
            .arg(path)
            .output()
            .expect("the bfr binary runs");
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let _ = writeln!(failures, "{name}: bfr failed with {}\n{stderr}", output.status);
            continue;
        }
        let actual = String::from_utf8_lossy(&output.stdout).into_owned();
        let expected = uncomment(&lines[case.expect.clone()]);

        let mut edits = Vec::new();
        if printed != input {
            edits.push((case.input.clone(), printed.clone()));
        }
        if actual != expected {
            edits.push((case.expect.clone(), comment(&actual)));
        }
        if edits.is_empty() {
            continue;
        }
        if updating {
            fs::write(path, splice(&lines, &edits)).unwrap_or_else(|e| panic!("{name}: {e}"));
            continue;
        }
        if printed != input {
            let _ = writeln!(failures, "{name}: input is not written as the printer prints it");
            let _ = writeln!(failures, "--- in the file\n{input}--- as printed\n{printed}");
        }
        if actual != expected {
            let _ = writeln!(failures, "{name}: output differs from the expected block");
            let _ = writeln!(failures, "--- expected\n{expected}--- actual\n{actual}");
        }
    }
    assert!(failures.is_empty(), "\n{failures}");
}
