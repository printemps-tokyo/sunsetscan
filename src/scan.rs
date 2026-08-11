//! Walking a repository and feeding files to the [`Matcher`].
//!
//! This is the only module that touches the filesystem. The walk is
//! gitignore-aware (`.gitignore`, `.ignore`, `.git/info/exclude`, the global
//! gitignore and any parent's), the same behavior repotome and agentsdoc have,
//! so a scan sees what the repository actually tracks.

use std::path::Path;

use ignore::overrides::OverrideBuilder;
use ignore::WalkBuilder;

use crate::matcher::{FileHits, Matcher};
use crate::model::ScanStats;

/// Files larger than this are skipped. Source files are not this big; the
/// things that are -- minified bundles, lockfiles, fixtures, checked-in data --
/// produce noise rather than call sites.
pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// How much of a file is inspected before deciding it is binary.
const BINARY_SNIFF_BYTES: usize = 8192;

/// Extensions scanned by default. Anything else needs `--all-files`.
///
/// This list is deliberately wide: sunsetscan looks for model ids and REST
/// paths, and those turn up in configuration, infrastructure and CI files at
/// least as often as in application code.
pub const SOURCE_EXTENSIONS: &[&str] = &[
    "astro",
    "bash",
    "c",
    "cc",
    "cfg",
    "cjs",
    "clj",
    "conf",
    "cpp",
    "cs",
    "css",
    "cxx",
    "dart",
    "elm",
    "env",
    "erb",
    "ex",
    "exs",
    "fish",
    "go",
    "gradle",
    "graphql",
    "groovy",
    "h",
    "handlebars",
    "hbs",
    "hcl",
    "hpp",
    "hs",
    "html",
    "ini",
    "ipynb",
    "java",
    "jinja",
    "js",
    "json",
    "json5",
    "jsonc",
    "jsx",
    "kt",
    "kts",
    "lua",
    "m",
    "md",
    "mdx",
    "mjs",
    "mm",
    "nix",
    "php",
    "pl",
    "properties",
    "proto",
    "ps1",
    "py",
    "pyi",
    "r",
    "rb",
    "rs",
    "sbt",
    "scala",
    "scm",
    "sh",
    "sql",
    "svelte",
    "swift",
    "tcl",
    "tf",
    "tfvars",
    "tmpl",
    "toml",
    "tpl",
    "ts",
    "tsx",
    "txt",
    "vue",
    "xml",
    "yaml",
    "yml",
    "zsh",
];

/// Extensionless files scanned by default.
pub const SOURCE_FILENAMES: &[&str] = &[
    "Brewfile",
    "Caddyfile",
    "Gemfile",
    "Justfile",
    "Makefile",
    "Procfile",
    "Rakefile",
    "justfile",
    "makefile",
];

/// Traversal settings.
#[derive(Debug, Clone, Default)]
pub struct ScanOptions {
    /// Scan every text file, not just the default source extensions.
    pub all_files: bool,
    /// gitignore-syntax globs whose matches are skipped.
    pub exclude: Vec<String>,
}

/// True when a repository-relative path is one of the default source kinds.
pub fn looks_like_source(rel_path: &str) -> bool {
    let name = rel_path.rsplit('/').next().unwrap_or(rel_path);
    if SOURCE_FILENAMES.contains(&name) {
        return true;
    }
    // Dockerfile, Dockerfile.dev, .env, .env.production, ...
    if name.starts_with("Dockerfile") || name.starts_with(".env") {
        return true;
    }
    match name.rsplit_once('.') {
        // A leading dot is the whole name (".gitignore"), not an extension.
        Some((stem, ext)) if !stem.is_empty() => SOURCE_EXTENSIONS.contains(&ext),
        _ => false,
    }
}

/// True when `bytes` look like a binary file.
fn looks_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(BINARY_SNIFF_BYTES)].contains(&0)
}

/// Whether ignore files *above* `root` should be consulted.
///
/// The `ignore` crate's `parents(true)` ascends to the filesystem root, which
/// is wrong when `root` is itself a repository: a `.gitignore` above a
/// repository does not apply to it, and a workspace directory holding a bare
/// `*` would otherwise make every repository inside it scan as completely
/// empty. When `root` is a repository root, its own ignore files are all that
/// matter and the ascent is switched off.
///
/// When `root` is a subdirectory -- `sunsetscan src` inside a checkout -- the
/// ascent stays on, because the repository's root `.gitignore` lives up there
/// and is exactly what the caller expects to be honored.
fn wants_parent_ignores(root: &Path) -> bool {
    !root.join(".git").exists()
}

/// Walk `root` and run `matcher` over every file that survives the filters.
///
/// Returns the per-file hits (in walk order; [`crate::model::Report::build`]
/// sorts them) and the counts of what was skipped and why.
pub fn scan_tree(
    root: &Path,
    opts: &ScanOptions,
    matcher: &Matcher,
) -> Result<(Vec<FileHits>, ScanStats), String> {
    let mut overrides = OverrideBuilder::new(root);
    for glob in &opts.exclude {
        // Override treats a bare glob as an allowlist, so an exclusion is the
        // negated form. This gives --exclude full gitignore glob syntax and
        // lets the walker prune whole directories.
        overrides
            .add(&format!("!{glob}"))
            .map_err(|e| format!("invalid --exclude glob '{glob}': {e}"))?;
    }
    let overrides = overrides
        .build()
        .map_err(|e| format!("invalid --exclude globs: {e}"))?;

    let mut walker = WalkBuilder::new(root);
    walker
        // Dotfiles are scanned: .github workflows and .env files name models.
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        // See `wants_parent_ignores`.
        .parents(wants_parent_ignores(root))
        // Honor .gitignore even when the target is not inside a git repo.
        .require_git(false)
        .add_custom_ignore_filename(".sunsetscanignore")
        .overrides(overrides)
        .filter_entry(|e| e.file_name() != ".git");

    let mut out = Vec::new();
    let mut stats = ScanStats::default();

    for result in walker.build() {
        let dent = match result {
            Ok(d) => d,
            // A directory we cannot read is not a scan failure; note it and
            // carry on, so one unreadable path does not lose the whole report.
            Err(_) => {
                stats.skipped_unreadable += 1;
                continue;
            }
        };
        if !dent.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = dent.path();
        let rel = match path.strip_prefix(root) {
            Ok(r) => r.to_string_lossy().replace('\\', "/"),
            Err(_) => path.to_string_lossy().replace('\\', "/"),
        };

        if !opts.all_files && !looks_like_source(&rel) {
            stats.skipped_extension += 1;
            continue;
        }
        match dent.metadata() {
            Ok(md) if md.len() > MAX_FILE_BYTES => {
                stats.skipped_large += 1;
                continue;
            }
            Ok(_) => {}
            Err(_) => {
                stats.skipped_unreadable += 1;
                continue;
            }
        }

        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(_) => {
                stats.skipped_unreadable += 1;
                continue;
            }
        };
        if looks_binary(&bytes) {
            stats.skipped_binary += 1;
            continue;
        }
        let Ok(text) = String::from_utf8(bytes) else {
            stats.skipped_binary += 1;
            continue;
        };

        stats.scanned += 1;
        let hits = matcher.scan(&rel, &text);
        if !hits.is_empty() {
            out.push((rel, hits));
        }
    }

    Ok((out, stats))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_detection_covers_extensions_names_and_dotfiles() {
        for yes in [
            "a.py",
            "src/deep/a.ts",
            "infra/main.tf",
            ".github/workflows/ci.yml",
            "Dockerfile",
            "Dockerfile.dev",
            "docker/Dockerfile",
            ".env",
            ".env.production",
            "Makefile",
            "notes.md",
        ] {
            assert!(looks_like_source(yes), "{yes} should be scanned");
        }
        for no in [
            "a.png",
            "a.woff2",
            "logo.svg",
            ".gitignore",
            "LICENSE",
            "bin/tool",
            "archive.tar.gz",
        ] {
            assert!(!looks_like_source(no), "{no} should be skipped");
        }
    }

    #[test]
    fn binary_sniffing_keys_on_nul_bytes() {
        assert!(!looks_binary(b"print('hello')\n"));
        assert!(looks_binary(b"\x7fELF\x02\x01\x01\x00"));
        assert!(!looks_binary(b""));
        // A NUL past the sniff window does not count.
        let mut late = vec![b'a'; BINARY_SNIFF_BYTES + 10];
        late[BINARY_SNIFF_BYTES + 5] = 0;
        assert!(!looks_binary(&late));
    }

    #[test]
    fn the_extension_list_is_sorted_and_free_of_duplicates() {
        // Sorted so that adding one stays a clean diff.
        let mut sorted = SOURCE_EXTENSIONS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.as_slice(), SOURCE_EXTENSIONS);
    }
}
