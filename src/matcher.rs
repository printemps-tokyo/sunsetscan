//! Textual matching of catalog patterns against file contents.
//!
//! This is the honest core of the tool, and the honesty matters: sunsetscan
//! does **not** parse any language. It matches literal strings, line by line,
//! with an optional boundary rule. That means a hit inside a comment, a
//! docstring, a changelog or a vendored copy of somebody else's SDK is a real
//! hit as far as this module is concerned. `--exclude` and the
//! `sunsetscan:ignore` marker exist because of that, not in spite of it.
//!
//! Everything here is pure: strings in, [`Hit`]s out. No filesystem access.

use ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::catalog::{Catalog, MatchKind};

/// Longest snippet rendered for a hit, in characters.
pub const SNIPPET_MAX: usize = 120;

/// The inline marker that suppresses findings on the line it appears on.
pub const IGNORE_MARKER: &str = "sunsetscan:ignore";

/// One catalog pattern that fired on a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchedPattern {
    /// The literal that matched.
    pub text: String,
    pub kind: MatchKind,
    /// The pattern's `note`, if it had one.
    pub note: Option<String>,
    /// 1-based character column of the match.
    pub column: usize,
}

/// One line of one file that matched one catalog entry.
///
/// Several patterns from the same entry can fire on the same line (an SDK path
/// and a parameter name, say); they are collected here rather than reported as
/// separate hits, so the line is printed once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// Index into the [`Catalog`] the [`Matcher`] was built from.
    pub entry_index: usize,
    /// 1-based line number.
    pub line: usize,
    /// The trimmed, length-capped source line.
    pub snippet: String,
    /// Patterns that fired, ordered by column then by text.
    pub patterns: Vec<MatchedPattern>,
}

/// Every hit in one file, keyed by its repository-relative path.
pub type FileHits = (String, Vec<Hit>);

impl Hit {
    /// 1-based column of the leftmost match on the line.
    pub fn column(&self) -> usize {
        self.patterns.first().map_or(1, |p| p.column)
    }
}

/// A catalog with its scope globs compiled, ready to run over file contents.
pub struct Matcher {
    entries: Vec<CompiledEntry>,
    /// Every catalog id, used to interpret a scoped `sunsetscan:ignore`.
    ids: Vec<String>,
}

struct CompiledEntry {
    index: usize,
    id: String,
    patterns: Vec<CompiledPattern>,
}

struct CompiledPattern {
    text: String,
    kind: MatchKind,
    note: Option<String>,
    /// `None` when the pattern applies to every scanned file.
    scope: Option<Gitignore>,
}

impl Matcher {
    /// Compile `catalog`. Fails only if a pattern's `scope` contains a glob the
    /// gitignore glob engine rejects.
    pub fn new(catalog: &Catalog) -> Result<Matcher, String> {
        let mut entries = Vec::with_capacity(catalog.entries.len());
        for (index, entry) in catalog.entries.iter().enumerate() {
            let mut patterns = Vec::with_capacity(entry.patterns.len());
            for pattern in &entry.patterns {
                let scope =
                    if pattern.scope.is_empty() {
                        None
                    } else {
                        Some(compile_globs(&pattern.scope).map_err(|e| {
                            format!("entry '{}': invalid scope glob: {e}", entry.id)
                        })?)
                    };
                patterns.push(CompiledPattern {
                    text: pattern.text.clone(),
                    kind: pattern.kind,
                    note: pattern.note.clone(),
                    scope,
                });
            }
            entries.push(CompiledEntry {
                index,
                id: entry.id.clone(),
                patterns,
            });
        }
        let ids = catalog.entries.iter().map(|e| e.id.clone()).collect();
        Ok(Matcher { entries, ids })
    }

    /// Run every pattern over `content`.
    ///
    /// `rel_path` is the repository-relative path with forward slashes; it is
    /// only used to evaluate pattern scopes. The returned hits are ordered by
    /// entry index, then line.
    pub fn scan(&self, rel_path: &str, content: &str) -> Vec<Hit> {
        let mut hits = Vec::new();
        for entry in &self.entries {
            let applicable: Vec<&CompiledPattern> = entry
                .patterns
                .iter()
                .filter(|p| in_scope(p, rel_path))
                .collect();
            if applicable.is_empty() {
                continue;
            }
            for (i, raw) in content.lines().enumerate() {
                // `lines()` strips \n but leaves \r on CRLF input.
                let line = raw.strip_suffix('\r').unwrap_or(raw);
                let suppression = Suppression::parse(line, &self.ids);
                if suppression.covers(&entry.id) {
                    continue;
                }
                let mut matched: Vec<MatchedPattern> = Vec::new();
                for p in &applicable {
                    if let Some(byte_offset) = find_first(line, &p.text, p.kind) {
                        matched.push(MatchedPattern {
                            text: p.text.clone(),
                            kind: p.kind,
                            note: p.note.clone(),
                            column: line[..byte_offset].chars().count() + 1,
                        });
                    }
                }
                if matched.is_empty() {
                    continue;
                }
                matched.sort_by(|a, b| a.column.cmp(&b.column).then_with(|| a.text.cmp(&b.text)));
                let focus = matched[0].column;
                hits.push(Hit {
                    entry_index: entry.index,
                    line: i + 1,
                    snippet: snippet(line, focus),
                    patterns: matched,
                });
            }
        }
        hits
    }
}

fn in_scope(pattern: &CompiledPattern, rel_path: &str) -> bool {
    match &pattern.scope {
        None => true,
        Some(globs) => globs.matched(rel_path, false).is_ignore(),
    }
}

fn compile_globs(globs: &[String]) -> Result<Gitignore, ignore::Error> {
    // An empty root makes the matcher operate directly on the relative paths
    // handed to `matched`, which is what the scanner already produces.
    let mut builder = GitignoreBuilder::new("");
    for glob in globs {
        builder.add_line(None, glob)?;
    }
    builder.build()
}

/// Find the first occurrence of `needle` in `haystack` that satisfies `kind`'s
/// boundary rule, returning its byte offset.
pub fn find_first(haystack: &str, needle: &str, kind: MatchKind) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    for (offset, _) in haystack.match_indices(needle) {
        if kind == MatchKind::Substring {
            return Some(offset);
        }
        let before = haystack[..offset].chars().next_back();
        let first = needle.chars().next();
        if !kind.boundary_between(before, first) {
            continue;
        }
        let end = offset + needle.len();
        let after = haystack[end..].chars().next();
        let last = needle.chars().next_back();
        if !kind.boundary_between(last, after) {
            continue;
        }
        return Some(offset);
    }
    None
}

/// What an inline `sunsetscan:ignore` marker on a line suppresses.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Suppression {
    /// No marker on this line.
    None,
    /// A bare marker: everything on this line is suppressed.
    All,
    /// `sunsetscan:ignore <id>[, <id>]`: only those entries are suppressed.
    Only(Vec<String>),
}

impl Suppression {
    /// Read the marker, if any.
    ///
    /// The tokens after the marker are treated as entry ids only when every one
    /// of them names an entry in the catalog. Anything else -- prose, a reason,
    /// a mistyped id -- degrades to the bare form and suppresses the whole
    /// line, which is the behavior a reader of the comment expects.
    fn parse(line: &str, known_ids: &[String]) -> Suppression {
        let Some(at) = line.find(IGNORE_MARKER) else {
            return Suppression::None;
        };
        let rest = &line[at + IGNORE_MARKER.len()..];
        let mut ids = Vec::new();
        for token in rest.split([',', ' ', '\t']) {
            let token = token.trim();
            if token.is_empty() {
                continue;
            }
            if !known_ids.iter().any(|id| id == token) {
                return Suppression::All;
            }
            ids.push(token.to_string());
        }
        if ids.is_empty() {
            Suppression::All
        } else {
            Suppression::Only(ids)
        }
    }

    fn covers(&self, entry_id: &str) -> bool {
        match self {
            Suppression::None => false,
            Suppression::All => true,
            Suppression::Only(ids) => ids.iter().any(|id| id == entry_id),
        }
    }
}

/// Trim a line and cap it at [`SNIPPET_MAX`] characters, keeping the character
/// at `focus` (a 1-based column) visible.
fn snippet(line: &str, focus: usize) -> String {
    let chars: Vec<char> = line.chars().collect();
    let start = chars.iter().position(|c| !c.is_whitespace()).unwrap_or(0);
    let end = chars
        .iter()
        .rposition(|c| !c.is_whitespace())
        .map_or(start, |i| i + 1);
    let trimmed = &chars[start..end];
    if trimmed.len() <= SNIPPET_MAX {
        return trimmed.iter().collect();
    }

    // Long line: slide a window so the match sits a third of the way in.
    let focus_in_trimmed = focus.saturating_sub(1).saturating_sub(start);
    let lead = SNIPPET_MAX / 3;
    let mut from = focus_in_trimmed.saturating_sub(lead);
    if from + SNIPPET_MAX > trimmed.len() {
        from = trimmed.len() - SNIPPET_MAX;
    }
    let to = from + SNIPPET_MAX;

    let mut out = String::new();
    if from > 0 {
        out.push_str("...");
    }
    out.extend(&trimmed[from..to]);
    if to < trimmed.len() {
        out.push_str("...");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;

    fn catalog(json: &str) -> Catalog {
        Catalog::from_json(json, "test").unwrap()
    }

    fn one_pattern(text: &str, kind: &str) -> Catalog {
        catalog(&format!(
            r#"{{"schema": 1, "verified_on": "2026-01-01", "entries": [
                {{"id": "e", "vendor": "V", "name": "N", "sunset": "2026-12-01",
                  "source_url": "https://e/x", "verified_on": "2026-01-01",
                  "replacement": "R", "migration": "M",
                  "patterns": [{{"text": "{text}", "kind": "{kind}"}}]}}]}}"#
        ))
    }

    #[test]
    fn word_boundaries_reject_longer_identifiers() {
        // The documented example: assistant_id must not fire on
        // my_assistant_identifier.
        assert_eq!(
            find_first("my_assistant_identifier", "assistant_id", MatchKind::Word),
            None
        );
        assert_eq!(
            find_first("x = assistant_id", "assistant_id", MatchKind::Word),
            Some(4)
        );
        assert_eq!(
            find_first(
                "runs.create(assistant_id=a)",
                "assistant_id",
                MatchKind::Word
            ),
            Some(12)
        );
        assert_eq!(
            find_first("assistant_ids", "assistant_id", MatchKind::Word),
            None
        );
        assert_eq!(
            find_first("xassistant_id", "assistant_id", MatchKind::Word),
            None
        );
        // Substring semantics do fire, which is exactly the difference.
        assert_eq!(
            find_first(
                "my_assistant_identifier",
                "assistant_id",
                MatchKind::Substring
            ),
            Some(3)
        );
    }

    #[test]
    fn word_boundaries_skip_a_bad_hit_and_take_a_later_good_one() {
        let line = "my_assistant_identifier and assistant_id";
        assert_eq!(find_first(line, "assistant_id", MatchKind::Word), Some(28));
    }

    #[test]
    fn token_boundaries_separate_neighbouring_model_ids() {
        // gpt-image-1 and gpt-image-1.5 are different models with different
        // shutdown dates, so the shorter id must not match the longer one.
        for longer in ["gpt-image-1.5", "gpt-image-1-mini", "gpt-image-15"] {
            assert_eq!(
                find_first(longer, "gpt-image-1", MatchKind::Token),
                None,
                "{longer}"
            );
        }
        assert_eq!(
            find_first(r#"model="gpt-image-1","#, "gpt-image-1", MatchKind::Token),
            Some(7)
        );
        assert_eq!(
            find_first("gpt-image-1", "gpt-image-1", MatchKind::Token),
            Some(0)
        );
        // Under word semantics '-' is a boundary, so the same pattern would
        // wrongly fire inside the longer id. This is why `token` exists.
        assert_eq!(
            find_first("gpt-image-1-mini", "gpt-image-1", MatchKind::Word),
            Some(0)
        );
    }

    #[test]
    fn matching_is_case_sensitive() {
        assert_eq!(
            find_first("Assistant_Id", "assistant_id", MatchKind::Word),
            None
        );
        assert_eq!(
            find_first("BETA.THREADS", "beta.threads", MatchKind::Substring),
            None
        );
    }

    #[test]
    fn substring_patterns_match_rest_paths_anywhere() {
        let m = Matcher::new(&one_pattern("/v1/threads", "substring")).unwrap();
        let hits = m.scan("a.js", "fetch('https://api.openai.com/v1/threads')");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].line, 1);
        assert_eq!(hits[0].patterns[0].text, "/v1/threads");
        assert_eq!(hits[0].column(), 30);
    }

    #[test]
    fn several_patterns_on_one_line_collapse_into_one_hit() {
        let cat = catalog(
            r#"{"schema": 1, "verified_on": "2026-01-01", "entries": [
                {"id": "e", "vendor": "V", "name": "N", "sunset": "2026-12-01",
                 "source_url": "https://e/x", "verified_on": "2026-01-01",
                 "replacement": "R", "migration": "M", "patterns": [
                   {"text": "beta.threads", "kind": "substring"},
                   {"text": "assistant_id", "kind": "word"}]}]}"#,
        );
        let m = Matcher::new(&cat).unwrap();
        let hits = m.scan("a.py", "client.beta.threads.runs.create(assistant_id=x)");
        assert_eq!(hits.len(), 1);
        let texts: Vec<&str> = hits[0].patterns.iter().map(|p| p.text.as_str()).collect();
        assert_eq!(texts, ["beta.threads", "assistant_id"]);
        assert!(hits[0].patterns[0].column < hits[0].patterns[1].column);
    }

    #[test]
    fn a_pattern_reports_only_its_first_occurrence_on_a_line() {
        let m = Matcher::new(&one_pattern("/v1/threads", "substring")).unwrap();
        let hits = m.scan("a.js", "'/v1/threads' + '/v1/threads'");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].patterns.len(), 1);
    }

    #[test]
    fn line_numbers_are_one_based_and_survive_crlf() {
        let m = Matcher::new(&one_pattern("assistant_id", "word")).unwrap();
        let hits = m.scan("a.py", "a\r\nb\r\nassistant_id = 1\r\n");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].line, 3);
        assert_eq!(hits[0].snippet, "assistant_id = 1");
    }

    #[test]
    fn a_bare_ignore_marker_suppresses_the_whole_line() {
        let m = Matcher::new(&one_pattern("assistant_id", "word")).unwrap();
        assert!(m
            .scan("a.py", "assistant_id = 1  # sunsetscan:ignore")
            .is_empty());
        assert_eq!(m.scan("a.py", "assistant_id = 1").len(), 1);
    }

    #[test]
    fn a_scoped_ignore_marker_only_suppresses_the_named_entry() {
        let cat = catalog(
            r#"{"schema": 1, "verified_on": "2026-01-01", "entries": [
                {"id": "aaa", "vendor": "V", "name": "N", "sunset": "2026-12-01",
                 "source_url": "https://e/x", "verified_on": "2026-01-01",
                 "replacement": "R", "migration": "M",
                 "patterns": [{"text": "alpha", "kind": "word"}]},
                {"id": "bbb", "vendor": "V", "name": "N", "sunset": "2026-12-02",
                 "source_url": "https://e/x", "verified_on": "2026-01-01",
                 "replacement": "R", "migration": "M",
                 "patterns": [{"text": "beta", "kind": "word"}]}]}"#,
        );
        let m = Matcher::new(&cat).unwrap();
        let hits = m.scan("a.py", "alpha beta  # sunsetscan:ignore aaa");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].entry_index, 1);

        let hits = m.scan("a.py", "alpha beta  # sunsetscan:ignore aaa, bbb");
        assert!(hits.is_empty());
    }

    #[test]
    fn an_unrecognised_word_after_the_marker_falls_back_to_suppress_all() {
        let m = Matcher::new(&one_pattern("assistant_id", "word")).unwrap();
        for line in [
            "assistant_id = 1  # sunsetscan:ignore legacy admin tool",
            "assistant_id = 1  # sunsetscan:ignore -- typoed-id",
            "assistant_id = 1  // sunsetscan:ignore: see TICKET-12",
        ] {
            assert!(m.scan("a.py", line).is_empty(), "{line}");
        }
    }

    #[test]
    fn scope_globs_restrict_a_pattern_to_matching_paths() {
        let cat = catalog(
            r#"{"schema": 1, "verified_on": "2026-01-01", "entries": [
                {"id": "e", "vendor": "V", "name": "N", "sunset": "2026-12-01",
                 "source_url": "https://e/x", "verified_on": "2026-01-01",
                 "replacement": "R", "migration": "M", "patterns": [
                   {"text": "needle", "kind": "word", "scope": ["*.py", "src/**/*.ts"]}]}]}"#,
        );
        let m = Matcher::new(&cat).unwrap();
        assert_eq!(m.scan("a.py", "needle").len(), 1);
        assert_eq!(m.scan("deep/nested/a.py", "needle").len(), 1);
        assert_eq!(m.scan("src/x/y.ts", "needle").len(), 1);
        assert_eq!(m.scan("a.js", "needle").len(), 0);
        assert_eq!(m.scan("other/y.ts", "needle").len(), 0);
    }

    #[test]
    fn hits_come_back_in_entry_then_line_order() {
        let cat = catalog(
            r#"{"schema": 1, "verified_on": "2026-01-01", "entries": [
                {"id": "aaa", "vendor": "V", "name": "N", "sunset": "2026-12-01",
                 "source_url": "https://e/x", "verified_on": "2026-01-01",
                 "replacement": "R", "migration": "M",
                 "patterns": [{"text": "alpha", "kind": "word"}]},
                {"id": "bbb", "vendor": "V", "name": "N", "sunset": "2026-12-02",
                 "source_url": "https://e/x", "verified_on": "2026-01-01",
                 "replacement": "R", "migration": "M",
                 "patterns": [{"text": "beta", "kind": "word"}]}]}"#,
        );
        let m = Matcher::new(&cat).unwrap();
        let hits = m.scan("a.py", "beta\nalpha\nbeta\nalpha\n");
        let got: Vec<(usize, usize)> = hits.iter().map(|h| (h.entry_index, h.line)).collect();
        assert_eq!(got, [(0, 2), (0, 4), (1, 1), (1, 3)]);
    }

    #[test]
    fn snippets_are_trimmed_and_capped_around_the_match() {
        assert_eq!(snippet("    hello    ", 5), "hello");
        let long = format!("{}NEEDLE{}", "x".repeat(400), "y".repeat(400));
        let s = snippet(&long, 401);
        assert!(
            s.chars().count() <= SNIPPET_MAX + 6,
            "{}",
            s.chars().count()
        );
        assert!(s.contains("NEEDLE"), "{s}");
        assert!(s.starts_with("..."), "{s}");
        assert!(s.ends_with("..."), "{s}");
        // A match near the end still shows, without an out-of-range window.
        let tail = format!("{}NEEDLE", "x".repeat(400));
        let s = snippet(&tail, 401);
        assert!(s.ends_with("NEEDLE"), "{s}");
    }

    #[test]
    fn snippets_respect_character_boundaries() {
        let line = format!("{}assistant_id", "あ".repeat(200));
        let s = snippet(&line, 201);
        assert!(s.contains("assistant_id"), "{s}");
        assert!(s.chars().count() <= SNIPPET_MAX + 6);
    }

    #[test]
    fn columns_count_characters_not_bytes() {
        let m = Matcher::new(&one_pattern("assistant_id", "word")).unwrap();
        let hits = m.scan("a.py", "# 日本語 assistant_id");
        assert_eq!(hits[0].column(), 7);
    }

    #[test]
    fn the_builtin_catalog_compiles() {
        let m = Matcher::new(&Catalog::builtin()).unwrap();
        let hits = m.scan(
            "app.py",
            "client.beta.assistants.create(model=\"gpt-4-turbo\")",
        );
        assert_eq!(hits.len(), 2, "{hits:#?}");
    }
}
