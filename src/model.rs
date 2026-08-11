//! The report model: how raw hits become a grouped, ordered, scored report.
//!
//! Pure data plus the rollup. Nothing here touches the filesystem, so the whole
//! reporting contract -- grouping, ordering, urgency buckets, exit codes -- is
//! unit-testable from in-memory inputs.

use std::fmt;

use crate::catalog::{Catalog, Entry};
use crate::date::Date;
use crate::matcher::{FileHits, MatchedPattern};

/// Findings at or inside this many days of the sunset are [`Urgency::Critical`],
/// and are what the default exit-code policy fails on.
pub const CRITICAL_DAYS: i64 = 30;

/// Findings at or inside this many days of the sunset are at least
/// [`Urgency::Warning`].
pub const WARNING_DAYS: i64 = 90;

/// The framing printed under every report and every `--list`.
pub const CAVEAT: &str = "Catalog verified as of the date above; vendors move dates. Confirm \
against the source_url before you act on a finding. Matching is textual, not semantic: hits in \
comments, documentation and vendored code are reported like any other.";

/// How close a sunset is, relative to the run's `--as-of` date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Urgency {
    /// The shutdown date has already passed.
    PastDue,
    /// Within [`CRITICAL_DAYS`] days, inclusive.
    Critical,
    /// Within [`WARNING_DAYS`] days, inclusive.
    Warning,
    /// Further out than [`WARNING_DAYS`].
    Later,
}

impl Urgency {
    /// Bucket a day count. Zero means "shuts down today", which is critical,
    /// not past due.
    pub fn for_days(days: i64) -> Urgency {
        if days < 0 {
            Urgency::PastDue
        } else if days <= CRITICAL_DAYS {
            Urgency::Critical
        } else if days <= WARNING_DAYS {
            Urgency::Warning
        } else {
            Urgency::Later
        }
    }

    /// Fixed-width label used in the human report.
    pub fn label(self) -> &'static str {
        match self {
            Urgency::PastDue => "PAST DUE",
            Urgency::Critical => "<=30d   ",
            Urgency::Warning => "<=90d   ",
            Urgency::Later => "later   ",
        }
    }

    /// Machine token used in JSON output.
    pub fn as_str(self) -> &'static str {
        match self {
            Urgency::PastDue => "past_due",
            Urgency::Critical => "critical",
            Urgency::Warning => "warning",
            Urgency::Later => "later",
        }
    }

    /// ANSI SGR parameters for the human report.
    pub fn color(self) -> &'static str {
        match self {
            Urgency::PastDue => "1;31",
            Urgency::Critical => "31",
            Urgency::Warning => "33",
            Urgency::Later => "2",
        }
    }
}

impl fmt::Display for Urgency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// When a run should exit non-zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailPolicy {
    /// Fail when any finding's sunset is `days` or fewer days away, past dues
    /// included. This is the default, with `days` = [`CRITICAL_DAYS`].
    Within(i64),
    /// `--strict`: fail on any finding at all, however distant.
    Any,
}

impl Default for FailPolicy {
    fn default() -> FailPolicy {
        FailPolicy::Within(CRITICAL_DAYS)
    }
}

/// One matched line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineFinding {
    /// 1-based.
    pub line: usize,
    /// 1-based character column of the leftmost match.
    pub column: usize,
    /// The trimmed, capped source line.
    pub snippet: String,
    /// Which patterns fired, left to right.
    pub patterns: Vec<MatchedPattern>,
}

/// Every matched line in one file, for one catalog entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileFindings {
    /// Repository-relative path with forward slashes.
    pub path: String,
    /// Ascending by line.
    pub lines: Vec<LineFinding>,
}

/// Everything found for one catalog entry.
#[derive(Debug, Clone, PartialEq)]
pub struct EntryFindings {
    pub entry: Entry,
    /// Days from the run's `as_of` date to the sunset. Negative once passed.
    pub days_remaining: i64,
    pub urgency: Urgency,
    /// Ascending by path.
    pub files: Vec<FileFindings>,
    /// Total matched lines across every file.
    pub total: usize,
}

/// How many files the walk looked at and why it skipped the rest.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanStats {
    /// Files whose contents were matched against the catalog.
    pub scanned: usize,
    /// Skipped because the extension is not in the default source set.
    pub skipped_extension: usize,
    /// Skipped because they were larger than the size cap.
    pub skipped_large: usize,
    /// Skipped because they were binary or not valid UTF-8.
    pub skipped_binary: usize,
    /// Skipped because they could not be read.
    pub skipped_unreadable: usize,
}

impl ScanStats {
    pub fn skipped_total(&self) -> usize {
        self.skipped_extension + self.skipped_large + self.skipped_binary + self.skipped_unreadable
    }
}

/// The finished report.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    /// Display name of the scanned directory.
    pub root_name: String,
    /// The date days-remaining was computed from.
    pub as_of: Date,
    /// The oldest `verified_on` across every catalog source in play.
    pub catalog_verified_on: Date,
    /// How many catalog entries were actually searched for.
    pub entries_searched: usize,
    /// Entries with at least one finding, ascending by sunset then id.
    pub entries: Vec<EntryFindings>,
    pub stats: ScanStats,
}

impl Report {
    /// Group raw hits into the report.
    ///
    /// `hits` is `(repository-relative path, hits in that file)`. Ordering of
    /// the input does not matter: everything is sorted here, so callers cannot
    /// produce unstable output by walking the tree in a different order.
    pub fn build(
        root_name: String,
        as_of: Date,
        catalog: &Catalog,
        hits: Vec<FileHits>,
        stats: ScanStats,
    ) -> Report {
        // entry index -> path -> lines
        let mut per_entry: Vec<Vec<(String, Vec<LineFinding>)>> =
            vec![Vec::new(); catalog.entries.len()];

        for (path, file_hits) in hits {
            for hit in file_hits {
                let Some(bucket) = per_entry.get_mut(hit.entry_index) else {
                    continue;
                };
                let finding = LineFinding {
                    line: hit.line,
                    column: hit.column(),
                    snippet: hit.snippet,
                    patterns: hit.patterns,
                };
                match bucket.iter_mut().find(|(p, _)| *p == path) {
                    Some((_, lines)) => lines.push(finding),
                    None => bucket.push((path.clone(), vec![finding])),
                }
            }
        }

        let mut entries = Vec::new();
        for (index, entry) in catalog.entries.iter().enumerate() {
            let bucket = &mut per_entry[index];
            if bucket.is_empty() {
                continue;
            }
            bucket.sort_by(|a, b| a.0.cmp(&b.0));
            let mut total = 0;
            let files: Vec<FileFindings> = bucket
                .drain(..)
                .map(|(path, mut lines)| {
                    lines.sort_by_key(|l| (l.line, l.column));
                    total += lines.len();
                    FileFindings { path, lines }
                })
                .collect();
            let days_remaining = entry.days_remaining(as_of);
            entries.push(EntryFindings {
                entry: entry.clone(),
                days_remaining,
                urgency: Urgency::for_days(days_remaining),
                files,
                total,
            });
        }
        // catalog.entries is already sunset-then-id ordered, so `entries` is too.

        Report {
            root_name,
            as_of,
            catalog_verified_on: catalog.verified_on,
            entries_searched: catalog.entries.len(),
            entries,
            stats,
        }
    }

    /// Total matched lines across every entry.
    pub fn total_findings(&self) -> usize {
        self.entries.iter().map(|e| e.total).sum()
    }

    /// Number of distinct files with at least one finding.
    pub fn files_with_findings(&self) -> usize {
        let mut paths: Vec<&str> = self
            .entries
            .iter()
            .flat_map(|e| e.files.iter().map(|f| f.path.as_str()))
            .collect();
        paths.sort_unstable();
        paths.dedup();
        paths.len()
    }

    /// The matched entry whose sunset is soonest, if anything matched.
    pub fn nearest(&self) -> Option<&EntryFindings> {
        self.entries.iter().min_by_key(|e| e.days_remaining)
    }

    /// True when `policy` considers this report a failure.
    pub fn breached(&self, policy: FailPolicy) -> bool {
        match policy {
            FailPolicy::Any => !self.entries.is_empty(),
            FailPolicy::Within(days) => self.entries.iter().any(|e| e.days_remaining <= days),
        }
    }

    /// Process exit code: 0 clean, 1 threshold breached. Usage and I/O errors
    /// exit 2 from `main`, never from here.
    pub fn exit_code(&self, policy: FailPolicy) -> i32 {
        i32::from(self.breached(policy))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::MatchKind;
    use crate::matcher::Hit;

    fn catalog() -> Catalog {
        Catalog::from_json(
            r#"{"schema": 1, "verified_on": "2026-01-01", "entries": [
                {"id": "soon", "vendor": "V", "name": "Soon", "sunset": "2026-08-26",
                 "source_url": "https://e/x", "verified_on": "2026-01-01",
                 "replacement": "R", "migration": "M",
                 "patterns": [{"text": "alpha", "kind": "word"}]},
                {"id": "later", "vendor": "V", "name": "Later", "sunset": "2027-06-01",
                 "source_url": "https://e/x", "verified_on": "2026-01-01",
                 "replacement": "R", "migration": "M",
                 "patterns": [{"text": "beta", "kind": "word"}]}]}"#,
            "test",
        )
        .unwrap()
    }

    fn hit(entry_index: usize, line: usize) -> Hit {
        Hit {
            entry_index,
            line,
            snippet: format!("line {line}"),
            patterns: vec![MatchedPattern {
                text: "alpha".into(),
                kind: MatchKind::Word,
                note: None,
                column: 3,
            }],
        }
    }

    fn report(hits: Vec<(String, Vec<Hit>)>, as_of: &str) -> Report {
        Report::build(
            "repo".into(),
            Date::parse(as_of).unwrap(),
            &catalog(),
            hits,
            ScanStats::default(),
        )
    }

    #[test]
    fn urgency_buckets_have_inclusive_boundaries() {
        assert_eq!(Urgency::for_days(-1), Urgency::PastDue);
        assert_eq!(Urgency::for_days(0), Urgency::Critical);
        assert_eq!(Urgency::for_days(30), Urgency::Critical);
        assert_eq!(Urgency::for_days(31), Urgency::Warning);
        assert_eq!(Urgency::for_days(90), Urgency::Warning);
        assert_eq!(Urgency::for_days(91), Urgency::Later);
    }

    #[test]
    fn as_of_drives_days_remaining_and_the_bucket() {
        let hits = vec![("a.py".to_string(), vec![hit(0, 1)])];
        let r = report(hits.clone(), "2026-08-11");
        assert_eq!(r.entries[0].days_remaining, 15);
        assert_eq!(r.entries[0].urgency, Urgency::Critical);

        let r = report(hits.clone(), "2026-09-01");
        assert_eq!(r.entries[0].days_remaining, -6);
        assert_eq!(r.entries[0].urgency, Urgency::PastDue);

        let r = report(hits, "2026-01-01");
        assert_eq!(r.entries[0].days_remaining, 237);
        assert_eq!(r.entries[0].urgency, Urgency::Later);
    }

    #[test]
    fn findings_group_by_entry_then_file_then_line() {
        let r = report(
            vec![
                ("z.py".into(), vec![hit(0, 9), hit(0, 2)]),
                ("a.py".into(), vec![hit(1, 4), hit(0, 7)]),
            ],
            "2026-08-11",
        );
        assert_eq!(r.entries.len(), 2);
        // Catalog order is sunset-ascending, so "soon" comes first.
        assert_eq!(r.entries[0].entry.id, "soon");
        assert_eq!(r.entries[1].entry.id, "later");

        let paths: Vec<&str> = r.entries[0].files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["a.py", "z.py"]);
        let lines: Vec<usize> = r.entries[0].files[1].lines.iter().map(|l| l.line).collect();
        assert_eq!(lines, [2, 9]);

        assert_eq!(r.total_findings(), 4);
        assert_eq!(r.files_with_findings(), 2);
    }

    #[test]
    fn a_clean_scan_reports_nothing_and_has_no_nearest() {
        let r = report(vec![("a.py".into(), vec![])], "2026-08-11");
        assert!(r.entries.is_empty());
        assert_eq!(r.total_findings(), 0);
        assert!(r.nearest().is_none());
        assert_eq!(r.entries_searched, 2);
    }

    #[test]
    fn nearest_is_the_soonest_matched_entry() {
        let r = report(vec![("a.py".into(), vec![hit(1, 1)])], "2026-08-11");
        assert_eq!(r.nearest().unwrap().entry.id, "later");
        let r = report(
            vec![("a.py".into(), vec![hit(0, 1), hit(1, 1)])],
            "2026-08-11",
        );
        assert_eq!(r.nearest().unwrap().entry.id, "soon");
    }

    #[test]
    fn fail_within_is_inclusive_at_the_boundary() {
        // "soon" sunsets 2026-08-26; as of 2026-08-11 that is 15 days out.
        let r = report(vec![("a.py".into(), vec![hit(0, 1)])], "2026-08-11");
        assert!(!r.breached(FailPolicy::Within(14)));
        assert!(r.breached(FailPolicy::Within(15)));
        assert!(r.breached(FailPolicy::Within(16)));
        assert_eq!(r.exit_code(FailPolicy::Within(14)), 0);
        assert_eq!(r.exit_code(FailPolicy::Within(15)), 1);
    }

    #[test]
    fn a_past_due_finding_breaches_every_threshold_including_zero() {
        let r = report(vec![("a.py".into(), vec![hit(0, 1)])], "2026-09-01");
        assert!(r.breached(FailPolicy::Within(0)));
        assert!(r.breached(FailPolicy::default()));
    }

    #[test]
    fn distant_findings_pass_by_default_but_fail_under_strict() {
        let r = report(vec![("a.py".into(), vec![hit(1, 1)])], "2026-08-11");
        assert_eq!(r.total_findings(), 1);
        assert!(!r.breached(FailPolicy::default()));
        assert!(r.breached(FailPolicy::Any));
        assert_eq!(r.exit_code(FailPolicy::default()), 0);
        assert_eq!(r.exit_code(FailPolicy::Any), 1);
    }

    #[test]
    fn strict_on_a_clean_report_still_exits_zero() {
        let r = report(vec![], "2026-08-11");
        assert_eq!(r.exit_code(FailPolicy::Any), 0);
    }

    #[test]
    fn scan_stats_sum_their_skips() {
        let stats = ScanStats {
            scanned: 10,
            skipped_extension: 1,
            skipped_large: 2,
            skipped_binary: 3,
            skipped_unreadable: 4,
        };
        assert_eq!(stats.skipped_total(), 10);
    }
}
