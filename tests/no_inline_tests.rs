//! Repo convention: every test lives in `tests/`, never in a `#[cfg(test)]`
//! module inside `src/`.
//!
//! Why bother pinning it: `src/audio/viz.rs` carried an 8-test `mod tests` for
//! a while, which contradicted the documented layout and ran as part of the
//! `lib`/`main` unit-test binaries rather than as a suite you can name and
//! filter. It is a one-line grep to regress, so it gets a test.

use std::path::{Path, PathBuf};

fn src_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            src_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_cfg_test_modules_in_src() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("src");
    let mut files = Vec::new();
    src_files(&root, &mut files);
    assert!(
        files.len() >= 20,
        "expected the whole src tree, found {}",
        files.len()
    );

    let mut offenders = Vec::new();
    for file in &files {
        for (n, line) in std::fs::read_to_string(file).unwrap().lines().enumerate() {
            // Skip comments: a doc comment is allowed to *mention* `#[test]`
            // (audio/transition.rs does, describing what the arm is for).
            let code = line.trim_start();
            if code.starts_with("//") {
                continue;
            }
            if code.contains("#[cfg(test)]")
                || code.contains("#[test]")
                || code.contains("mod tests")
            {
                offenders.push(format!("{}:{}: {}", file.display(), n + 1, code.trim()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "tests must live in tests/, not in src/:\n  {}",
        offenders.join("\n  ")
    );
}
