//! Filesystem-level tests: what the walk sees and what it refuses to look at.
//!
//! These drive the library directly (`scan_tree`) so the filters can be checked
//! independently of argument parsing and rendering.

use std::fs;
use std::path::Path;

use sunsetscan::catalog::Catalog;
use sunsetscan::matcher::Matcher;
use sunsetscan::scan::{scan_tree, ScanOptions, MAX_FILE_BYTES};
use tempfile::TempDir;

fn fixture(files: &[(&str, &str)]) -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    for (path, contents) in files {
        write(dir.path(), path, contents);
    }
    dir
}

fn write(root: &Path, rel: &str, contents: &str) {
    let full = root.join(rel);
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent).expect("mkdir");
    }
    fs::write(&full, contents).expect("write");
}

/// Scan a tree and return the matched paths, sorted.
fn paths(dir: &TempDir, opts: &ScanOptions) -> Vec<String> {
    let catalog = Catalog::builtin();
    let matcher = Matcher::new(&catalog).unwrap();
    let (hits, _) = scan_tree(dir.path(), opts, &matcher).unwrap();
    let mut got: Vec<String> = hits.into_iter().map(|(p, _)| p).collect();
    got.sort();
    got
}

fn stats(dir: &TempDir, opts: &ScanOptions) -> sunsetscan::model::ScanStats {
    let catalog = Catalog::builtin();
    let matcher = Matcher::new(&catalog).unwrap();
    scan_tree(dir.path(), opts, &matcher).unwrap().1
}

const HIT: &str = "resp = client.beta.threads.runs.create(assistant_id=A)\n";

#[test]
fn finds_hits_across_nested_directories() {
    let dir = fixture(&[
        ("app.py", HIT),
        ("src/deep/worker.py", HIT),
        (
            "web/client.ts",
            "fetch('https://api.openai.com/v1/threads')\n",
        ),
        ("README.md", "We use the Assistants API.\n"),
    ]);
    assert_eq!(
        paths(&dir, &ScanOptions::default()),
        ["app.py", "src/deep/worker.py", "web/client.ts"]
    );
}

#[test]
fn a_clean_repository_produces_nothing() {
    let dir = fixture(&[
        (
            "app.py",
            "from openai import OpenAI\nclient.responses.create()\n",
        ),
        ("web/client.ts", "await client.conversations.create();\n"),
        ("README.md", "Nothing to see here.\n"),
    ]);
    let got = paths(&dir, &ScanOptions::default());
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn gitignored_paths_are_skipped() {
    let dir = fixture(&[
        (".gitignore", "vendor/\n*.generated.py\n"),
        ("app.py", HIT),
        ("vendor/sdk.py", HIT),
        ("thing.generated.py", HIT),
    ]);
    assert_eq!(paths(&dir, &ScanOptions::default()), ["app.py"]);
}

#[test]
fn the_git_directory_is_never_walked() {
    let dir = fixture(&[("app.py", HIT), (".git/hooks/pre-commit", HIT)]);
    assert_eq!(paths(&dir, &ScanOptions::default()), ["app.py"]);
}

#[test]
fn a_dedicated_ignore_file_is_honored() {
    let dir = fixture(&[
        (".sunsetscanignore", "legacy/\n"),
        ("app.py", HIT),
        ("legacy/old.py", HIT),
    ]);
    assert_eq!(paths(&dir, &ScanOptions::default()), ["app.py"]);
}

#[test]
fn dotfile_directories_such_as_github_are_scanned() {
    let dir = fixture(&[(".github/workflows/ci.yml", "  MODEL: gpt-4-turbo\n")]);
    assert_eq!(
        paths(&dir, &ScanOptions::default()),
        [".github/workflows/ci.yml"]
    );
}

#[test]
fn exclude_globs_prune_files_and_whole_directories() {
    let dir = fixture(&[
        ("app.py", HIT),
        ("tests/test_app.py", HIT),
        ("docs/notes.md", "assistant_id\n"),
    ]);
    assert_eq!(
        paths(
            &dir,
            &ScanOptions {
                exclude: vec!["tests/**".into()],
                ..ScanOptions::default()
            }
        ),
        ["app.py", "docs/notes.md"]
    );
    assert_eq!(
        paths(
            &dir,
            &ScanOptions {
                exclude: vec!["tests/**".into(), "*.md".into()],
                ..ScanOptions::default()
            }
        ),
        ["app.py"]
    );
    assert_eq!(
        paths(
            &dir,
            &ScanOptions {
                exclude: vec!["**/*.py".into()],
                ..ScanOptions::default()
            }
        ),
        ["docs/notes.md"]
    );
}

#[test]
fn an_invalid_exclude_glob_is_an_error_not_a_panic() {
    let dir = fixture(&[("app.py", HIT)]);
    let catalog = Catalog::builtin();
    let matcher = Matcher::new(&catalog).unwrap();
    let opts = ScanOptions {
        exclude: vec!["[".into()],
        ..ScanOptions::default()
    };
    let err = scan_tree(dir.path(), &opts, &matcher).unwrap_err();
    assert!(err.contains("--exclude"), "{err}");
}

#[test]
fn unknown_extensions_need_all_files() {
    let dir = fixture(&[("data.parquetish", HIT), ("app.py", HIT)]);
    assert_eq!(paths(&dir, &ScanOptions::default()), ["app.py"]);
    assert_eq!(
        stats(&dir, &ScanOptions::default()).skipped_extension,
        1,
        "the skip should be counted, not silent"
    );

    let all = ScanOptions {
        all_files: true,
        ..ScanOptions::default()
    };
    assert_eq!(paths(&dir, &all), ["app.py", "data.parquetish"]);
    assert_eq!(stats(&dir, &all).skipped_extension, 0);
}

#[test]
fn binary_files_are_skipped_even_with_all_files() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("app.py"), HIT).unwrap();
    // A NUL byte next to text that would otherwise match.
    let mut blob = HIT.as_bytes().to_vec();
    blob.insert(0, 0);
    fs::write(dir.path().join("blob.py"), &blob).unwrap();
    // Invalid UTF-8 counts as binary too.
    fs::write(dir.path().join("latin.py"), [0xff, 0xfe, b'a']).unwrap();

    let opts = ScanOptions {
        all_files: true,
        ..ScanOptions::default()
    };
    assert_eq!(paths(&dir, &opts), ["app.py"]);
    assert_eq!(stats(&dir, &opts).skipped_binary, 2);
}

#[test]
fn files_over_the_size_cap_are_skipped() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("app.py"), HIT).unwrap();

    let mut big = String::with_capacity(MAX_FILE_BYTES as usize + 4096);
    big.push_str(HIT);
    while big.len() as u64 <= MAX_FILE_BYTES {
        big.push_str("# padding padding padding padding padding padding padding\n");
    }
    fs::write(dir.path().join("huge.py"), &big).unwrap();
    assert!(big.len() as u64 > MAX_FILE_BYTES);

    let opts = ScanOptions::default();
    assert_eq!(paths(&dir, &opts), ["app.py"]);
    let s = stats(&dir, &opts);
    assert_eq!(s.skipped_large, 1);
    assert_eq!(s.scanned, 1);
}

#[test]
fn a_file_just_under_the_cap_is_still_scanned() {
    let dir = tempfile::tempdir().unwrap();
    const PAD: &str = "# padding\n";
    let room = MAX_FILE_BYTES as usize - HIT.len();
    let mut nearly = PAD.repeat(room / PAD.len());
    nearly.push_str(HIT);
    assert!(nearly.len() as u64 <= MAX_FILE_BYTES);
    fs::write(dir.path().join("big.py"), &nearly).unwrap();
    assert_eq!(paths(&dir, &ScanOptions::default()), ["big.py"]);
}

#[test]
fn the_inline_marker_suppresses_a_line_on_disk_too() {
    let dir = fixture(&[
        ("kept.py", HIT),
        (
            "muted.py",
            "resp = client.beta.threads.create()  # sunsetscan:ignore\n",
        ),
    ]);
    assert_eq!(paths(&dir, &ScanOptions::default()), ["kept.py"]);
}

#[test]
fn paths_are_reported_relative_with_forward_slashes() {
    let dir = fixture(&[("a/b/c/deep.py", HIT)]);
    assert_eq!(paths(&dir, &ScanOptions::default()), ["a/b/c/deep.py"]);
}

#[test]
fn an_empty_tree_scans_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let s = stats(&dir, &ScanOptions::default());
    assert_eq!(s.scanned, 0);
    assert_eq!(s.skipped_total(), 0);
}

#[test]
fn a_gitignore_above_the_repository_does_not_hide_it() {
    // A workspace directory that ignores everything must not blank out a
    // repository checked out inside it.
    let outer = tempfile::tempdir().unwrap();
    fs::write(outer.path().join(".gitignore"), "*\n").unwrap();
    let repo = outer.path().join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    fs::write(repo.join("app.py"), HIT).unwrap();

    let catalog = Catalog::builtin();
    let matcher = Matcher::new(&catalog).unwrap();
    let (hits, _) = scan_tree(&repo, &ScanOptions::default(), &matcher).unwrap();
    let got: Vec<String> = hits.into_iter().map(|(p, _)| p).collect();
    assert_eq!(got, ["app.py"]);
}

#[test]
fn scanning_a_subdirectory_still_honors_the_repository_root_gitignore() {
    let repo = tempfile::tempdir().unwrap();
    fs::create_dir_all(repo.path().join(".git")).unwrap();
    fs::write(repo.path().join(".gitignore"), "src/generated/\n").unwrap();
    write(repo.path(), "src/app.py", HIT);
    write(repo.path(), "src/generated/out.py", HIT);

    let catalog = Catalog::builtin();
    let matcher = Matcher::new(&catalog).unwrap();
    let (hits, _) = scan_tree(&repo.path().join("src"), &ScanOptions::default(), &matcher).unwrap();
    let got: Vec<String> = hits.into_iter().map(|(p, _)| p).collect();
    assert_eq!(got, ["app.py"]);
}
