//! Rendering: the human report, the `--list` view and the JSON document.
//!
//! Every renderer is a pure function of its input and wraps at a fixed width
//! rather than the terminal width, so identical input produces identical bytes
//! on every machine and in every CI run.

use crate::catalog::Catalog;
use crate::date::Date;
use crate::json::quote;
use crate::model::{Report, Urgency, CAVEAT};

/// Fixed wrap width. Not terminal-derived, on purpose.
const WRAP: usize = 92;

/// Column the detail fields' values start at.
const FIELD_WIDTH: usize = 13;

/// Rendering options for the human output.
#[derive(Debug, Clone, Copy, Default)]
pub struct RenderOptions {
    /// Emit ANSI color codes.
    pub color: bool,
}

fn paint(opts: RenderOptions, code: &str, s: &str) -> String {
    if opts.color {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

/// "15 days remaining", "1 day remaining", "today", "6 days ago".
pub fn days_phrase(days: i64) -> String {
    match days {
        0 => "today".to_string(),
        1 => "1 day remaining".to_string(),
        -1 => "1 day ago".to_string(),
        d if d > 0 => format!("{d} days remaining"),
        d => format!("{} days ago", -d),
    }
}

// ---------------------------------------------------------------------------
// Human report
// ---------------------------------------------------------------------------

/// The full human-readable report, including the trailing caveat.
pub fn render_text(report: &Report, opts: RenderOptions) -> String {
    let mut out = format!("sunsetscan: {}\n", report.root_name);
    out.push_str(&summary_line(report, opts));
    out.push('\n');

    if let Some(nearest) = report.nearest() {
        out.push_str(&format!(
            "nearest deadline: {}  {} ({})\n",
            nearest.entry.sunset,
            nearest.entry.name,
            days_phrase(nearest.days_remaining),
        ));
    }
    out.push('\n');

    if report.entries.is_empty() {
        out.push_str("No calls into a sunsetting API were found.\n\n");
    }

    for found in &report.entries {
        let entry = &found.entry;
        out.push_str(&format!(
            "[{}] {}  {}\n",
            paint(opts, found.urgency.color(), found.urgency.label()),
            paint(opts, "1", &entry.id),
            entry.name,
        ));
        field(&mut out, "vendor", &entry.vendor);
        field(
            &mut out,
            "sunset",
            &format!("{} ({})", entry.sunset, days_phrase(found.days_remaining)),
        );
        field(&mut out, "replacement", &entry.replacement);
        field(&mut out, "migration", &entry.migration);
        if let Some(url) = &entry.migration_url {
            field(&mut out, "guide", url);
        }
        if let Some(notes) = &entry.notes {
            field(&mut out, "notes", notes);
        }
        field(
            &mut out,
            "source",
            &format!("{} (verified {})", entry.source_url, entry.verified_on),
        );
        out.push_str(&format!(
            "  {} finding(s) in {} file(s)\n",
            found.total,
            found.files.len()
        ));

        for file in &found.files {
            out.push_str(&format!("  {}\n", paint(opts, "4", &file.path)));
            for line in &file.lines {
                out.push_str(&format!(
                    "    {}:{}  {}\n",
                    line.line, line.column, line.snippet
                ));
                for p in &line.patterns {
                    let note = match &p.note {
                        Some(n) => format!(" ({n})"),
                        None => String::new(),
                    };
                    out.push_str(&paint(
                        opts,
                        "2",
                        &format!("      matched {}{note}\n", p.text),
                    ));
                }
            }
        }
        out.push('\n');
    }

    out.push_str(&footer(report));
    out
}

/// The one-line summary used as the `--quiet` output and as the report header.
pub fn summary_line(report: &Report, opts: RenderOptions) -> String {
    let total = report.total_findings();
    let urgency = report
        .entries
        .iter()
        .map(|e| e.urgency)
        .min()
        .unwrap_or(Urgency::Later);
    let headline = if total == 0 {
        paint(opts, "32", "clean")
    } else {
        paint(opts, urgency.color(), urgency.label().trim())
    };
    format!(
        "as of {}  |  {}  |  {} finding(s) in {} file(s), {} of {} catalog entr(ies) matched",
        report.as_of,
        headline,
        total,
        report.files_with_findings(),
        report.entries.len(),
        report.entries_searched,
    )
}

fn footer(report: &Report) -> String {
    let mut out = format!(
        "{} file(s) scanned, {} skipped ({} not a source extension, {} too large, {} binary, \
{} unreadable).\n",
        report.stats.scanned,
        report.stats.skipped_total(),
        report.stats.skipped_extension,
        report.stats.skipped_large,
        report.stats.skipped_binary,
        report.stats.skipped_unreadable,
    );
    out.push_str(&format!(
        "Catalog verified as of {}.\n",
        report.catalog_verified_on
    ));
    for line in wrap(CAVEAT, WRAP) {
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// A `  label       value` line, wrapped with a hanging indent.
fn field(out: &mut String, label: &str, value: &str) {
    let width = FIELD_WIDTH;
    let indent = " ".repeat(FIELD_WIDTH + 2);
    for (i, line) in wrap(value, WRAP - FIELD_WIDTH - 2).into_iter().enumerate() {
        if i == 0 {
            out.push_str(&format!("  {label:<width$}{line}\n"));
        } else {
            out.push_str(&format!("{indent}{line}\n"));
        }
    }
}

/// Greedy word wrap. Words longer than `width` are left intact rather than
/// broken, because they are almost always URLs.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if current.is_empty() {
            current.push_str(word);
        } else if current.chars().count() + 1 + word.chars().count() <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(std::mem::take(&mut current));
            current.push_str(word);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

// ---------------------------------------------------------------------------
// --list
// ---------------------------------------------------------------------------

/// The `--list` view: every catalog entry with its date and days remaining.
pub fn render_list_text(catalog: &Catalog, as_of: Date, opts: RenderOptions) -> String {
    let mut out = format!(
        "sunsetscan catalog: {} entr(ies), verified as of {}\nas of {}\n\n",
        catalog.entries.len(),
        catalog.verified_on,
        as_of
    );
    let id_width = catalog
        .entries
        .iter()
        .map(|e| e.id.chars().count())
        .max()
        .unwrap_or(2)
        .max(2);

    out.push_str(&format!(
        "  {:<8}  {:<10}  {:<width$}  {:<8}  {}\n",
        "STATUS",
        "SUNSET",
        "ID",
        "PATTERNS",
        "NAME",
        width = id_width
    ));
    for entry in &catalog.entries {
        let days = entry.days_remaining(as_of);
        let urgency = Urgency::for_days(days);
        // Pad before painting, or the escape codes would count toward the
        // column width and break the alignment.
        let status = paint(opts, urgency.color(), urgency.label());
        out.push_str(&format!(
            "  {}  {:<10}  {:<width$}  {:<8}  {} ({})\n",
            status,
            entry.sunset,
            entry.id,
            entry.patterns.len(),
            entry.name,
            days_phrase(days),
            width = id_width
        ));
    }
    out.push('\n');
    for entry in &catalog.entries {
        out.push_str(&format!("{}\n", paint(opts, "1", &entry.id)));
        field(&mut out, "vendor", &entry.vendor);
        field(&mut out, "replacement", &entry.replacement);
        field(&mut out, "migration", &entry.migration);
        if let Some(url) = &entry.migration_url {
            field(&mut out, "guide", url);
        }
        field(
            &mut out,
            "source",
            &format!("{} (verified {})", entry.source_url, entry.verified_on),
        );
        let patterns: Vec<String> = entry
            .patterns
            .iter()
            .map(|p| format!("{} [{}]", p.text, p.kind.as_str()))
            .collect();
        field(&mut out, "patterns", &patterns.join(", "));
        out.push('\n');
    }
    out.push_str(&format!(
        "Catalog verified as of {}.\n",
        catalog.verified_on
    ));
    for line in wrap(CAVEAT, WRAP) {
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// The `--list --json` document.
pub fn render_list_json(catalog: &Catalog, as_of: Date) -> String {
    let mut out = String::with_capacity(8192);
    out.push_str("{\n");
    out.push_str("  \"tool\": \"sunsetscan\",\n");
    out.push_str(&format!(
        "  \"version\": {},\n",
        quote(env!("CARGO_PKG_VERSION"))
    ));
    out.push_str(&format!(
        "  \"schema\": {},\n",
        crate::catalog::SCHEMA_VERSION
    ));
    out.push_str(&format!("  \"as_of\": {},\n", quote(&as_of.to_string())));
    out.push_str(&format!(
        "  \"catalog_verified_on\": {},\n",
        quote(&catalog.verified_on.to_string())
    ));
    out.push_str("  \"entries\": [\n");
    for (i, entry) in catalog.entries.iter().enumerate() {
        let days = entry.days_remaining(as_of);
        out.push_str("    {\n");
        out.push_str(&format!("      \"id\": {},\n", quote(&entry.id)));
        out.push_str(&format!("      \"vendor\": {},\n", quote(&entry.vendor)));
        out.push_str(&format!("      \"name\": {},\n", quote(&entry.name)));
        out.push_str(&format!(
            "      \"sunset\": {},\n",
            quote(&entry.sunset.to_string())
        ));
        out.push_str(&format!("      \"days_remaining\": {days},\n"));
        out.push_str(&format!(
            "      \"urgency\": {},\n",
            quote(Urgency::for_days(days).as_str())
        ));
        out.push_str(&format!(
            "      \"source_url\": {},\n",
            quote(&entry.source_url)
        ));
        out.push_str(&format!(
            "      \"verified_on\": {},\n",
            quote(&entry.verified_on.to_string())
        ));
        out.push_str(&format!(
            "      \"replacement\": {},\n",
            quote(&entry.replacement)
        ));
        out.push_str(&format!(
            "      \"migration\": {},\n",
            quote(&entry.migration)
        ));
        out.push_str(&format!(
            "      \"migration_url\": {},\n",
            opt(entry.migration_url.as_deref())
        ));
        out.push_str(&format!(
            "      \"notes\": {},\n",
            opt(entry.notes.as_deref())
        ));
        out.push_str("      \"patterns\": [\n");
        for (j, p) in entry.patterns.iter().enumerate() {
            let scope: Vec<String> = p.scope.iter().map(|g| quote(g)).collect();
            out.push_str(&format!(
                "        {{\"text\": {}, \"kind\": {}, \"note\": {}, \"scope\": [{}]}}",
                quote(&p.text),
                quote(p.kind.as_str()),
                opt(p.note.as_deref()),
                scope.join(", ")
            ));
            out.push_str(sep(j, entry.patterns.len()));
        }
        out.push_str("      ]\n");
        out.push_str("    }");
        out.push_str(sep(i, catalog.entries.len()));
    }
    out.push_str("  ],\n");
    out.push_str(&format!("  \"caveat\": {}\n", quote(CAVEAT)));
    out.push_str("}\n");
    out
}

// ---------------------------------------------------------------------------
// JSON report
// ---------------------------------------------------------------------------

/// The machine-readable report.
pub fn render_json(report: &Report) -> String {
    let mut out = String::with_capacity(8192);
    out.push_str("{\n");
    out.push_str("  \"tool\": \"sunsetscan\",\n");
    out.push_str(&format!(
        "  \"version\": {},\n",
        quote(env!("CARGO_PKG_VERSION"))
    ));
    out.push_str(&format!(
        "  \"schema\": {},\n",
        crate::catalog::SCHEMA_VERSION
    ));
    out.push_str(&format!("  \"root\": {},\n", quote(&report.root_name)));
    out.push_str(&format!(
        "  \"as_of\": {},\n",
        quote(&report.as_of.to_string())
    ));
    out.push_str(&format!(
        "  \"catalog_verified_on\": {},\n",
        quote(&report.catalog_verified_on.to_string())
    ));
    out.push_str(&format!(
        "  \"entries_searched\": {},\n",
        report.entries_searched
    ));
    out.push_str(&format!(
        "  \"totals\": {{\"findings\": {}, \"files_with_findings\": {}, \"entries_matched\": {}}},\n",
        report.total_findings(),
        report.files_with_findings(),
        report.entries.len(),
    ));
    out.push_str(&format!(
        "  \"stats\": {{\"scanned\": {}, \"skipped_extension\": {}, \"skipped_large\": {}, \
\"skipped_binary\": {}, \"skipped_unreadable\": {}}},\n",
        report.stats.scanned,
        report.stats.skipped_extension,
        report.stats.skipped_large,
        report.stats.skipped_binary,
        report.stats.skipped_unreadable,
    ));
    match report.nearest() {
        Some(n) => out.push_str(&format!(
            "  \"nearest_deadline\": {{\"entry_id\": {}, \"sunset\": {}, \"days_remaining\": {}}},\n",
            quote(&n.entry.id),
            quote(&n.entry.sunset.to_string()),
            n.days_remaining,
        )),
        None => out.push_str("  \"nearest_deadline\": null,\n"),
    }

    out.push_str("  \"findings\": [\n");
    for (i, found) in report.entries.iter().enumerate() {
        let entry = &found.entry;
        out.push_str("    {\n");
        out.push_str("      \"entry\": {\n");
        out.push_str(&format!("        \"id\": {},\n", quote(&entry.id)));
        out.push_str(&format!("        \"vendor\": {},\n", quote(&entry.vendor)));
        out.push_str(&format!("        \"name\": {},\n", quote(&entry.name)));
        out.push_str(&format!(
            "        \"sunset\": {},\n",
            quote(&entry.sunset.to_string())
        ));
        out.push_str(&format!(
            "        \"days_remaining\": {},\n",
            found.days_remaining
        ));
        out.push_str(&format!(
            "        \"urgency\": {},\n",
            quote(found.urgency.as_str())
        ));
        out.push_str(&format!(
            "        \"source_url\": {},\n",
            quote(&entry.source_url)
        ));
        out.push_str(&format!(
            "        \"verified_on\": {},\n",
            quote(&entry.verified_on.to_string())
        ));
        out.push_str(&format!(
            "        \"replacement\": {},\n",
            quote(&entry.replacement)
        ));
        out.push_str(&format!(
            "        \"migration\": {},\n",
            quote(&entry.migration)
        ));
        out.push_str(&format!(
            "        \"migration_url\": {},\n",
            opt(entry.migration_url.as_deref())
        ));
        out.push_str(&format!(
            "        \"notes\": {}\n",
            opt(entry.notes.as_deref())
        ));
        out.push_str("      },\n");
        out.push_str(&format!("      \"count\": {},\n", found.total));
        out.push_str("      \"files\": [\n");
        for (j, file) in found.files.iter().enumerate() {
            out.push_str("        {\n");
            out.push_str(&format!("          \"path\": {},\n", quote(&file.path)));
            out.push_str(&format!("          \"count\": {},\n", file.lines.len()));
            out.push_str("          \"lines\": [\n");
            for (k, line) in file.lines.iter().enumerate() {
                let patterns: Vec<String> = line
                    .patterns
                    .iter()
                    .map(|p| {
                        format!(
                            "{{\"text\": {}, \"kind\": {}, \"note\": {}, \"column\": {}}}",
                            quote(&p.text),
                            quote(p.kind.as_str()),
                            opt(p.note.as_deref()),
                            p.column
                        )
                    })
                    .collect();
                out.push_str(&format!(
                    "            {{\"line\": {}, \"column\": {}, \"snippet\": {}, \"patterns\": [{}]}}",
                    line.line,
                    line.column,
                    quote(&line.snippet),
                    patterns.join(", ")
                ));
                out.push_str(sep(k, file.lines.len()));
            }
            out.push_str("          ]\n");
            out.push_str("        }");
            out.push_str(sep(j, found.files.len()));
        }
        out.push_str("      ]\n");
        out.push_str("    }");
        out.push_str(sep(i, report.entries.len()));
    }
    out.push_str("  ],\n");
    out.push_str(&format!("  \"caveat\": {}\n", quote(CAVEAT)));
    out.push_str("}\n");
    out
}

/// The separator after a JSON array element: a comma unless it is the last.
fn sep(index: usize, len: usize) -> &'static str {
    if index + 1 < len {
        ",\n"
    } else {
        "\n"
    }
}

fn opt(value: Option<&str>) -> String {
    match value {
        Some(s) => quote(s),
        None => "null".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::MatchKind;
    use crate::matcher::{Hit, MatchedPattern};
    use crate::model::{Report, ScanStats};

    fn catalog() -> Catalog {
        Catalog::from_json(
            r#"{"schema": 1, "verified_on": "2026-08-11", "entries": [
                {"id": "soon", "vendor": "V", "name": "Soon API", "sunset": "2026-08-26",
                 "source_url": "https://e/x", "verified_on": "2026-08-01",
                 "replacement": "New API", "migration": "Do the thing.",
                 "migration_url": "https://e/guide", "notes": "A note.",
                 "patterns": [{"text": "alpha", "kind": "word", "note": "sdk"}]}]}"#,
            "test",
        )
        .unwrap()
    }

    fn report(with_hits: bool) -> Report {
        let hits = if with_hits {
            vec![(
                "src/a.py".to_string(),
                vec![Hit {
                    entry_index: 0,
                    line: 12,
                    snippet: "x = alpha()".into(),
                    patterns: vec![MatchedPattern {
                        text: "alpha".into(),
                        kind: MatchKind::Word,
                        note: Some("sdk".into()),
                        column: 5,
                    }],
                }],
            )]
        } else {
            vec![]
        };
        Report::build(
            "repo".into(),
            Date::parse("2026-08-11").unwrap(),
            &catalog(),
            hits,
            ScanStats {
                scanned: 3,
                skipped_extension: 1,
                ..ScanStats::default()
            },
        )
    }

    #[test]
    fn days_phrase_reads_naturally_on_both_sides_of_zero() {
        assert_eq!(days_phrase(15), "15 days remaining");
        assert_eq!(days_phrase(1), "1 day remaining");
        assert_eq!(days_phrase(0), "today");
        assert_eq!(days_phrase(-1), "1 day ago");
        assert_eq!(days_phrase(-6), "6 days ago");
    }

    #[test]
    fn the_text_report_carries_every_required_element() {
        let text = render_text(&report(true), RenderOptions::default());
        for needle in [
            "sunsetscan: repo",
            "as of 2026-08-11",
            "nearest deadline: 2026-08-26",
            "[<=30d",
            "soon",
            "2026-08-26 (15 days remaining)",
            "New API",
            "Do the thing.",
            "https://e/guide",
            "https://e/x (verified 2026-08-01)",
            "src/a.py",
            "12:5  x = alpha()",
            "matched alpha (sdk)",
            "Catalog verified as of 2026-08-11.",
            "vendors move dates",
        ] {
            assert!(text.contains(needle), "missing {needle:?} in:\n{text}");
        }
    }

    #[test]
    fn a_clean_report_says_so_and_still_prints_the_caveat() {
        let text = render_text(&report(false), RenderOptions::default());
        assert!(text.contains("No calls into a sunsetting API were found."));
        assert!(text.contains("Catalog verified as of"));
        assert!(!text.contains("nearest deadline"));
    }

    #[test]
    fn color_is_opt_in_and_never_leaks_when_off() {
        let plain = render_text(&report(true), RenderOptions { color: false });
        assert!(!plain.contains('\x1b'));
        let colored = render_text(&report(true), RenderOptions { color: true });
        assert!(colored.contains('\x1b'));
    }

    #[test]
    fn rendering_is_deterministic() {
        let r = report(true);
        assert_eq!(
            render_text(&r, RenderOptions::default()),
            render_text(&r, RenderOptions::default())
        );
        assert_eq!(render_json(&r), render_json(&r));
    }

    #[test]
    fn the_json_report_parses_and_has_the_documented_shape() {
        let doc = crate::json::parse(&render_json(&report(true))).unwrap();
        assert_eq!(doc.get("tool").unwrap().as_str(), Some("sunsetscan"));
        assert_eq!(doc.get("as_of").unwrap().as_str(), Some("2026-08-11"));
        assert_eq!(
            doc.get("catalog_verified_on").unwrap().as_str(),
            Some("2026-08-11")
        );
        let totals = doc.get("totals").unwrap();
        assert_eq!(
            totals.get("findings"),
            Some(&crate::json::Value::Number(1.0))
        );
        let nearest = doc.get("nearest_deadline").unwrap();
        assert_eq!(nearest.get("entry_id").unwrap().as_str(), Some("soon"));
        assert_eq!(
            nearest.get("days_remaining"),
            Some(&crate::json::Value::Number(15.0))
        );

        let findings = doc.get("findings").unwrap().as_array().unwrap();
        assert_eq!(findings.len(), 1);
        let entry = findings[0].get("entry").unwrap();
        assert_eq!(entry.get("id").unwrap().as_str(), Some("soon"));
        assert_eq!(entry.get("urgency").unwrap().as_str(), Some("critical"));
        assert_eq!(entry.get("notes").unwrap().as_str(), Some("A note."));

        let files = findings[0].get("files").unwrap().as_array().unwrap();
        assert_eq!(files[0].get("path").unwrap().as_str(), Some("src/a.py"));
        let lines = files[0].get("lines").unwrap().as_array().unwrap();
        assert_eq!(
            lines[0].get("line"),
            Some(&crate::json::Value::Number(12.0))
        );
        let patterns = lines[0].get("patterns").unwrap().as_array().unwrap();
        assert_eq!(patterns[0].get("text").unwrap().as_str(), Some("alpha"));
        assert_eq!(patterns[0].get("kind").unwrap().as_str(), Some("word"));
    }

    #[test]
    fn a_clean_json_report_still_parses() {
        let doc = crate::json::parse(&render_json(&report(false))).unwrap();
        assert!(doc.get("nearest_deadline").unwrap().is_null());
        assert!(doc.get("findings").unwrap().as_array().unwrap().is_empty());
    }

    #[test]
    fn the_list_view_shows_dates_days_and_the_caveat() {
        let as_of = Date::parse("2026-08-11").unwrap();
        let text = render_list_text(&catalog(), as_of, RenderOptions::default());
        assert!(text.contains("2026-08-26"));
        assert!(text.contains("15 days remaining"));
        assert!(text.contains("alpha [word]"));
        assert!(text.contains("Catalog verified as of 2026-08-11."));
        assert!(text.contains("vendors move dates"));
    }

    #[test]
    fn the_list_json_parses_and_includes_patterns() {
        let as_of = Date::parse("2026-08-11").unwrap();
        let doc = crate::json::parse(&render_list_json(&catalog(), as_of)).unwrap();
        let entries = doc.get("entries").unwrap().as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].get("days_remaining"),
            Some(&crate::json::Value::Number(15.0))
        );
        let patterns = entries[0].get("patterns").unwrap().as_array().unwrap();
        assert_eq!(patterns[0].get("text").unwrap().as_str(), Some("alpha"));
        assert!(patterns[0]
            .get("scope")
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn the_builtin_catalog_renders_in_both_list_modes() {
        let as_of = Date::parse("2026-08-11").unwrap();
        let c = Catalog::builtin();
        let text = render_list_text(&c, as_of, RenderOptions::default());
        assert!(text.contains("openai-assistants-api"));
        assert!(crate::json::parse(&render_list_json(&c, as_of)).is_ok());
    }

    #[test]
    fn wrap_respects_the_width_and_keeps_urls_intact() {
        let lines = wrap("aaa bbb ccc ddd", 7);
        assert_eq!(lines, ["aaa bbb", "ccc ddd"]);
        let long = "https://example.com/a/very/long/path/that/exceeds/the/width";
        assert_eq!(wrap(long, 10), [long]);
        assert_eq!(wrap("", 10), [""]);
    }
}
