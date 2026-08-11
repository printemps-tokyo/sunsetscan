//! The catalog: which APIs are going away, when, and what to look for.
//!
//! The catalog is data, not code. The built-in one lives in `src/catalog.json`
//! and is embedded at build time, so adding an entry is a one-file change and
//! the same parser and validator handle both it and any file passed to
//! `--catalog`. See CONTRIBUTING.md for the rules an entry has to satisfy.

use crate::date::Date;
use crate::json::{self, Value};

/// The built-in catalog, embedded at build time.
pub const BUILTIN_JSON: &str = include_str!("catalog.json");

/// The only schema version this build understands.
pub const SCHEMA_VERSION: u32 = 1;

/// How a pattern's text is matched against a line.
///
/// The distinction exists because the three things sunsetscan looks for behave
/// differently. A REST path is a plain substring. An identifier such as
/// `assistant_id` must not match inside a longer identifier. A model id such as
/// `gpt-image-1` must not match inside `gpt-image-1.5`, which is a *different*
/// model with a *different* shutdown date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchKind {
    /// Plain substring. Matches anywhere, including inside a longer word.
    Substring,
    /// Substring anchored at both ends to a word boundary, where a word
    /// character is `[A-Za-z0-9_]`.
    Word,
    /// Substring anchored at both ends to a token boundary, where a token
    /// character is `[A-Za-z0-9_.-]`. Intended for model identifiers.
    Token,
}

impl MatchKind {
    /// Machine token used in JSON, in both directions.
    pub fn as_str(self) -> &'static str {
        match self {
            MatchKind::Substring => "substring",
            MatchKind::Word => "word",
            MatchKind::Token => "token",
        }
    }

    pub fn parse(s: &str) -> Option<MatchKind> {
        match s {
            "substring" => Some(MatchKind::Substring),
            "word" => Some(MatchKind::Word),
            "token" => Some(MatchKind::Token),
            _ => None,
        }
    }

    /// Whether `c` continues a token under these semantics.
    fn is_boundary_char(self, c: char) -> bool {
        match self {
            MatchKind::Substring => false,
            MatchKind::Word => c.is_ascii_alphanumeric() || c == '_',
            MatchKind::Token => c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-',
        }
    }

    /// True when a boundary exists between the optional characters `left` and
    /// `right`. A boundary exists unless both sides are token characters, which
    /// is exactly the `\b` rule specialised to this character class.
    pub fn boundary_between(self, left: Option<char>, right: Option<char>) -> bool {
        match (left, right) {
            (Some(l), Some(r)) => !(self.is_boundary_char(l) && self.is_boundary_char(r)),
            _ => true,
        }
    }
}

/// One literal to look for, plus an optional restriction on which files it
/// applies to.
#[derive(Debug, Clone, PartialEq)]
pub struct Pattern {
    /// The literal text. Matching is case-sensitive.
    pub text: String,
    pub kind: MatchKind,
    /// What this literal is, shown next to the hit (e.g. "REST endpoint").
    pub note: Option<String>,
    /// gitignore-syntax globs matched against the repository-relative path.
    /// Empty means "every scanned file".
    pub scope: Vec<String>,
}

/// One sunsetting API.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// Stable id, also the `--only` / `--skip` value.
    pub id: String,
    pub vendor: String,
    /// Human-readable product name.
    pub name: String,
    /// The announced shutdown date.
    pub sunset: Date,
    /// The vendor page the date was read from.
    pub source_url: String,
    /// The date `source_url` was read.
    pub verified_on: Date,
    /// What the vendor tells you to use instead.
    pub replacement: String,
    /// How to get there, summarised.
    pub migration: String,
    /// The vendor's migration guide, when one exists.
    pub migration_url: Option<String>,
    /// Anything else worth printing (notice periods, grace periods).
    pub notes: Option<String>,
    pub patterns: Vec<Pattern>,
}

impl Entry {
    /// Days from `as_of` to the sunset. Negative once the date has passed.
    pub fn days_remaining(&self, as_of: Date) -> i64 {
        as_of.days_until(self.sunset)
    }
}

/// A set of entries, kept in deterministic order.
#[derive(Debug, Clone, PartialEq)]
pub struct Catalog {
    /// The oldest file-level `verified_on` across every merged source. Printed
    /// as the "catalog verified as of" date, so merging a stale overlay makes
    /// the report say so rather than inheriting the built-in's freshness.
    pub verified_on: Date,
    /// Sorted by sunset date, then by id.
    pub entries: Vec<Entry>,
}

impl Catalog {
    /// The catalog compiled into this binary.
    ///
    /// # Panics
    ///
    /// Only if `src/catalog.json` is malformed, which the test suite rejects at
    /// build time.
    pub fn builtin() -> Catalog {
        Catalog::from_json(BUILTIN_JSON, "<built-in>")
            .expect("the built-in catalog is validated by the test suite")
    }

    /// Parse a catalog document. `origin` is used in error messages.
    pub fn from_json(text: &str, origin: &str) -> Result<Catalog, String> {
        let doc = json::parse(text).map_err(|e| format!("{origin}: {e}"))?;
        let ctx = |what: &str| format!("{origin}: {what}");

        if doc.as_object().is_none() {
            return Err(ctx(&format!(
                "the document must be an object, found a {}",
                doc.type_name()
            )));
        }
        known_keys(&doc, &["schema", "verified_on", "entries"], origin, "")?;

        match doc.get("schema") {
            Some(Value::Number(n)) if *n == f64::from(SCHEMA_VERSION) => {}
            Some(Value::Number(n)) => {
                return Err(ctx(&format!(
                    "unsupported catalog schema {n}; this build understands schema {SCHEMA_VERSION}"
                )))
            }
            _ => return Err(ctx(&format!("missing \"schema\": {SCHEMA_VERSION}"))),
        }

        let verified_on = date_field(&doc, "verified_on", origin, "")?;

        let Some(items) = doc.get("entries").and_then(Value::as_array) else {
            return Err(ctx("missing \"entries\" array"));
        };

        let mut entries = Vec::with_capacity(items.len());
        for (i, item) in items.iter().enumerate() {
            entries.push(parse_entry(item, origin, i)?);
        }

        let mut catalog = Catalog {
            verified_on,
            entries,
        };
        catalog.finish(origin)?;
        Ok(catalog)
    }

    /// Sort into the canonical order and reject duplicate ids.
    fn finish(&mut self, origin: &str) -> Result<(), String> {
        self.entries
            .sort_by(|a, b| a.sunset.cmp(&b.sunset).then_with(|| a.id.cmp(&b.id)));
        for pair in self.entries.windows(2) {
            if pair[0].id == pair[1].id {
                return Err(format!("{origin}: duplicate entry id '{}'", pair[0].id));
            }
        }
        Ok(())
    }

    /// Fold `overlay` in. An entry whose id already exists replaces it
    /// wholesale; a new id is added. The catalog's `verified_on` becomes the
    /// older of the two.
    pub fn merge(&mut self, overlay: Catalog) {
        self.verified_on = self.verified_on.min(overlay.verified_on);
        for entry in overlay.entries {
            match self.entries.iter_mut().find(|e| e.id == entry.id) {
                Some(existing) => *existing = entry,
                None => self.entries.push(entry),
            }
        }
        // Ids were unique in both inputs and replacement preserves that, so
        // this cannot fail.
        let _ = self.finish("<merge>");
    }

    /// Apply `--only` and `--skip`. Unknown ids are an error rather than a
    /// silent no-op, because a typo would otherwise quietly widen the scan.
    pub fn retain(&mut self, only: &[String], skip: &[String]) -> Result<(), String> {
        for id in only.iter().chain(skip) {
            if !self.entries.iter().any(|e| &e.id == id) {
                return Err(format!(
                    "unknown catalog entry id '{id}'\nknown ids: {}",
                    self.ids().join(", ")
                ));
            }
        }
        self.entries
            .retain(|e| (only.is_empty() || only.contains(&e.id)) && !skip.contains(&e.id));
        Ok(())
    }

    /// Every id, in catalog order.
    pub fn ids(&self) -> Vec<&str> {
        self.entries.iter().map(|e| e.id.as_str()).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Field-level parsing
// ---------------------------------------------------------------------------

fn parse_entry(value: &Value, origin: &str, index: usize) -> Result<Entry, String> {
    // Errors are labelled by id where possible, because "entry 7" is useless
    // when someone reorders the file.
    let label = match value.get("id").and_then(Value::as_str) {
        Some(id) => format!("entry '{id}'"),
        None => format!("entry #{}", index + 1),
    };
    if value.as_object().is_none() {
        return Err(format!(
            "{origin}: {label} must be an object, found a {}",
            value.type_name()
        ));
    }
    known_keys(
        value,
        &[
            "id",
            "vendor",
            "name",
            "sunset",
            "source_url",
            "verified_on",
            "replacement",
            "migration",
            "migration_url",
            "notes",
            "patterns",
        ],
        origin,
        &label,
    )?;

    let id = string_field(value, "id", origin, &label)?;
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(format!(
            "{origin}: {label}: \"id\" must be non-empty and use only [a-z0-9-]"
        ));
    }

    let Some(raw_patterns) = value.get("patterns").and_then(Value::as_array) else {
        return Err(format!("{origin}: {label}: missing \"patterns\" array"));
    };
    if raw_patterns.is_empty() {
        return Err(format!(
            "{origin}: {label}: \"patterns\" must contain at least one pattern"
        ));
    }
    let mut patterns = Vec::with_capacity(raw_patterns.len());
    for (i, p) in raw_patterns.iter().enumerate() {
        patterns.push(parse_pattern(p, origin, &label, i)?);
    }

    Ok(Entry {
        id,
        vendor: string_field(value, "vendor", origin, &label)?,
        name: string_field(value, "name", origin, &label)?,
        sunset: date_field(value, "sunset", origin, &label)?,
        source_url: url_field(value, "source_url", origin, &label)?,
        verified_on: date_field(value, "verified_on", origin, &label)?,
        replacement: string_field(value, "replacement", origin, &label)?,
        migration: string_field(value, "migration", origin, &label)?,
        migration_url: optional_url_field(value, "migration_url", origin, &label)?,
        notes: optional_string_field(value, "notes", origin, &label)?,
        patterns,
    })
}

fn parse_pattern(
    value: &Value,
    origin: &str,
    entry: &str,
    index: usize,
) -> Result<Pattern, String> {
    let label = format!("{entry}, pattern #{}", index + 1);
    if value.as_object().is_none() {
        return Err(format!(
            "{origin}: {label} must be an object, found a {}",
            value.type_name()
        ));
    }
    known_keys(value, &["text", "kind", "note", "scope"], origin, &label)?;

    let text = string_field(value, "text", origin, &label)?;
    if text.is_empty() {
        return Err(format!("{origin}: {label}: \"text\" must not be empty"));
    }
    if text.contains('\n') {
        return Err(format!(
            "{origin}: {label}: \"text\" must not span lines; matching is line by line"
        ));
    }

    let kind_str = string_field(value, "kind", origin, &label)?;
    let Some(kind) = MatchKind::parse(&kind_str) else {
        return Err(format!(
            "{origin}: {label}: unknown \"kind\" '{kind_str}' (expected substring, word or token)"
        ));
    };

    let scope = match value.get("scope") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => {
            let mut globs = Vec::with_capacity(items.len());
            for item in items {
                match item.as_str() {
                    Some(g) => globs.push(g.to_string()),
                    None => {
                        return Err(format!(
                            "{origin}: {label}: every \"scope\" entry must be a glob string"
                        ))
                    }
                }
            }
            globs
        }
        Some(other) => {
            return Err(format!(
                "{origin}: {label}: \"scope\" must be an array of glob strings, found a {}",
                other.type_name()
            ))
        }
    };

    Ok(Pattern {
        text,
        kind,
        note: optional_string_field(value, "note", origin, &label)?,
        scope,
    })
}

/// Reject members the schema does not define, so a typo is loud.
fn known_keys(value: &Value, allowed: &[&str], origin: &str, label: &str) -> Result<(), String> {
    let Some(members) = value.as_object() else {
        return Ok(());
    };
    for (key, _) in members {
        if !allowed.contains(&key.as_str()) {
            let where_ = if label.is_empty() {
                String::new()
            } else {
                format!("{label}: ")
            };
            return Err(format!(
                "{origin}: {where_}unknown field \"{key}\" (allowed: {})",
                allowed.join(", ")
            ));
        }
    }
    Ok(())
}

fn string_field(value: &Value, key: &str, origin: &str, label: &str) -> Result<String, String> {
    match value.get(key) {
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(s.clone()),
        Some(Value::String(_)) => Err(field_err(origin, label, key, "must not be blank")),
        Some(other) => Err(field_err(
            origin,
            label,
            key,
            &format!("must be a string, found a {}", other.type_name()),
        )),
        None => Err(field_err(origin, label, key, "is required")),
    }
}

fn optional_string_field(
    value: &Value,
    key: &str,
    origin: &str,
    label: &str,
) -> Result<Option<String>, String> {
    match value.get(key) {
        None => Ok(None),
        Some(v) if v.is_null() => Ok(None),
        Some(_) => string_field(value, key, origin, label).map(Some),
    }
}

fn url_field(value: &Value, key: &str, origin: &str, label: &str) -> Result<String, String> {
    let s = string_field(value, key, origin, label)?;
    if !s.starts_with("https://") && !s.starts_with("http://") {
        return Err(field_err(origin, label, key, "must be an http(s) URL"));
    }
    Ok(s)
}

fn optional_url_field(
    value: &Value,
    key: &str,
    origin: &str,
    label: &str,
) -> Result<Option<String>, String> {
    match value.get(key) {
        None => Ok(None),
        Some(v) if v.is_null() => Ok(None),
        Some(_) => url_field(value, key, origin, label).map(Some),
    }
}

fn date_field(value: &Value, key: &str, origin: &str, label: &str) -> Result<Date, String> {
    let s = string_field(value, key, origin, label)?;
    Date::parse(&s).map_err(|e| field_err(origin, label, key, &e))
}

fn field_err(origin: &str, label: &str, key: &str, what: &str) -> String {
    if label.is_empty() {
        format!("{origin}: \"{key}\" {what}")
    } else {
        format!("{origin}: {label}: \"{key}\" {what}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal(extra: &str) -> String {
        format!(
            r#"{{
              "schema": 1,
              "verified_on": "2026-08-11",
              "entries": [
                {{
                  "id": "x-api",
                  "vendor": "X",
                  "name": "X API",
                  "sunset": "2026-12-01",
                  "source_url": "https://example.com/deprecations",
                  "verified_on": "2026-08-01",
                  "replacement": "Y API",
                  "migration": "Use Y."{extra},
                  "patterns": [{{"text": "/v1/x", "kind": "substring"}}]
                }}
              ]
            }}"#
        )
    }

    #[test]
    fn the_builtin_catalog_parses_and_validates() {
        let c = Catalog::builtin();
        assert!(!c.is_empty());
        for e in &c.entries {
            assert!(!e.patterns.is_empty(), "{} has no patterns", e.id);
            assert!(
                e.source_url.starts_with("https://"),
                "{} needs an https source_url",
                e.id
            );
            assert!(
                e.verified_on <= Date::parse("2100-01-01").unwrap(),
                "{} verified_on looks wrong",
                e.id
            );
        }
    }

    #[test]
    fn the_builtin_catalog_carries_the_assistants_entry() {
        let c = Catalog::builtin();
        let e = c
            .entries
            .iter()
            .find(|e| e.id == "openai-assistants-api")
            .expect("the motivating entry must ship");
        assert_eq!(e.sunset, Date::parse("2026-08-26").unwrap());
        assert_eq!(
            e.source_url,
            "https://developers.openai.com/api/docs/deprecations"
        );
        assert_eq!(
            e.migration_url.as_deref(),
            Some("https://developers.openai.com/api/docs/assistants/migration")
        );
        let texts: Vec<&str> = e.patterns.iter().map(|p| p.text.as_str()).collect();
        for expected in [
            "/v1/assistants",
            "/v1/threads",
            "beta.threads",
            "assistant_id",
        ] {
            assert!(texts.contains(&expected), "missing pattern {expected}");
        }
    }

    #[test]
    fn entries_are_sorted_by_sunset_then_id() {
        let c = Catalog::builtin();
        let mut sorted = c.entries.clone();
        sorted.sort_by(|a, b| a.sunset.cmp(&b.sunset).then_with(|| a.id.cmp(&b.id)));
        assert_eq!(c.entries, sorted);
    }

    #[test]
    fn optional_fields_default_to_none() {
        let c = Catalog::from_json(&minimal(""), "t").unwrap();
        assert_eq!(c.entries[0].migration_url, None);
        assert_eq!(c.entries[0].notes, None);
        assert!(c.entries[0].patterns[0].scope.is_empty());
        assert_eq!(c.entries[0].patterns[0].note, None);
    }

    #[test]
    fn explicit_nulls_are_treated_as_absent() {
        let c =
            Catalog::from_json(&minimal(r#", "migration_url": null, "notes": null"#), "t").unwrap();
        assert_eq!(c.entries[0].migration_url, None);
    }

    #[test]
    fn rejects_broken_catalogs_with_a_useful_message() {
        let cases: &[(&str, &str)] = &[
            (
                r#"{"schema": 2, "verified_on": "2026-01-01", "entries": []}"#,
                "schema",
            ),
            (r#"{"verified_on": "2026-01-01", "entries": []}"#, "schema"),
            (r#"{"schema": 1, "entries": []}"#, "verified_on"),
            (
                r#"{"schema": 1, "verified_on": "nope", "entries": []}"#,
                "YYYY-MM-DD",
            ),
            (r#"{"schema": 1, "verified_on": "2026-01-01"}"#, "entries"),
            (r#"[]"#, "must be an object"),
        ];
        for (doc, needle) in cases {
            let err = Catalog::from_json(doc, "t").unwrap_err();
            assert!(err.contains(needle), "{doc} -> {err}");
        }
    }

    #[test]
    fn rejects_unknown_fields_and_bad_entry_shapes() {
        let cases: &[(&str, &str)] = &[
            (r#", "sunsett": "2026-01-01""#, "unknown field"),
            (r#", "id": """#, "must not be blank"),
            (r#", "id": "Bad Id""#, "[a-z0-9-]"),
            (r#", "source_url": "ftp://x/y""#, "http(s) URL"),
            (r#", "sunset": "2026-99-01""#, "not a real calendar date"),
            (r#", "vendor": 3"#, "must be a string"),
            (r#", "migration": "   ""#, "must not be blank"),
        ];
        for (extra, needle) in cases {
            let err = Catalog::from_json(&minimal(extra), "t").unwrap_err();
            assert!(err.contains(needle), "{extra} -> {err}");
            assert!(err.contains("x-api") || err.contains("entry"), "{err}");
        }
    }

    #[test]
    fn rejects_bad_patterns() {
        let cases: &[(&str, &str)] = &[
            (r#"[]"#, "at least one pattern"),
            (r#"[{"text": "a", "kind": "regex"}]"#, "unknown \"kind\""),
            (r#"[{"text": "", "kind": "word"}]"#, "must not be blank"),
            (
                r#"[{"text": "a\nb", "kind": "word"}]"#,
                "must not span lines",
            ),
            (
                r#"[{"text": "a", "kind": "word", "scope": "*.py"}]"#,
                "array of glob",
            ),
            (
                r#"[{"text": "a", "kind": "word", "scope": [7]}]"#,
                "glob string",
            ),
            (r#"[{"text": "a"}]"#, "\"kind\" is required"),
            (r#"["a"]"#, "must be an object"),
        ];
        for (patterns, needle) in cases {
            let doc = format!(
                r#"{{"schema": 1, "verified_on": "2026-01-01", "entries": [
                    {{"id": "x", "vendor": "X", "name": "X", "sunset": "2026-12-01",
                      "source_url": "https://e/x", "verified_on": "2026-01-01",
                      "replacement": "Y", "migration": "m", "patterns": {patterns}}}]}}"#
            );
            let err = Catalog::from_json(&doc, "t").unwrap_err();
            assert!(err.contains(needle), "{patterns} -> {err}");
        }
    }

    #[test]
    fn rejects_duplicate_ids() {
        let doc = r#"{"schema": 1, "verified_on": "2026-01-01", "entries": [
            {"id": "x", "vendor": "X", "name": "X", "sunset": "2026-12-01",
             "source_url": "https://e/x", "verified_on": "2026-01-01",
             "replacement": "Y", "migration": "m", "patterns": [{"text":"a","kind":"word"}]},
            {"id": "x", "vendor": "X", "name": "X", "sunset": "2027-12-01",
             "source_url": "https://e/x", "verified_on": "2026-01-01",
             "replacement": "Y", "migration": "m", "patterns": [{"text":"b","kind":"word"}]}]}"#;
        let err = Catalog::from_json(doc, "t").unwrap_err();
        assert!(err.contains("duplicate entry id 'x'"), "{err}");
    }

    #[test]
    fn merge_replaces_by_id_and_keeps_the_older_verified_on() {
        let mut base = Catalog::builtin();
        let before = base.entries.len();
        let overlay = Catalog::from_json(
            r#"{"schema": 1, "verified_on": "2025-01-01", "entries": [
                {"id": "openai-assistants-api", "vendor": "OpenAI", "name": "override",
                 "sunset": "2030-01-01", "source_url": "https://e/x",
                 "verified_on": "2025-01-01", "replacement": "Y", "migration": "m",
                 "patterns": [{"text": "zzz", "kind": "word"}]},
                {"id": "brand-new", "vendor": "V", "name": "New", "sunset": "2029-01-01",
                 "source_url": "https://e/y", "verified_on": "2025-01-01",
                 "replacement": "Y", "migration": "m",
                 "patterns": [{"text": "qqq", "kind": "word"}]}]}"#,
            "overlay",
        )
        .unwrap();
        base.merge(overlay);

        assert_eq!(base.entries.len(), before + 1);
        assert_eq!(base.verified_on, Date::parse("2025-01-01").unwrap());
        let replaced = base
            .entries
            .iter()
            .find(|e| e.id == "openai-assistants-api")
            .unwrap();
        assert_eq!(replaced.name, "override");
        assert_eq!(replaced.patterns.len(), 1);
        assert!(base.entries.iter().any(|e| e.id == "brand-new"));
        // Still sorted after the merge.
        let mut sorted = base.entries.clone();
        sorted.sort_by(|a, b| a.sunset.cmp(&b.sunset).then_with(|| a.id.cmp(&b.id)));
        assert_eq!(base.entries, sorted);
    }

    #[test]
    fn retain_applies_only_and_skip_and_rejects_typos() {
        let mut c = Catalog::builtin();
        c.retain(
            &["openai-assistants-api".into(), "openai-videos-api".into()],
            &["openai-videos-api".into()],
        )
        .unwrap();
        assert_eq!(c.ids(), ["openai-assistants-api"]);

        let mut c = Catalog::builtin();
        let err = c.retain(&["nope".into()], &[]).unwrap_err();
        assert!(err.contains("unknown catalog entry id 'nope'"), "{err}");
        assert!(err.contains("openai-assistants-api"), "{err}");
    }

    #[test]
    fn boundary_rules_match_the_documented_semantics() {
        // Substring never anchors.
        assert!(MatchKind::Substring.boundary_between(Some('a'), Some('b')));
        // Word: '_' continues an identifier, '.' does not.
        assert!(!MatchKind::Word.boundary_between(Some('_'), Some('a')));
        assert!(MatchKind::Word.boundary_between(Some('.'), Some('a')));
        assert!(MatchKind::Word.boundary_between(None, Some('a')));
        // Token: '.' and '-' continue a model id.
        assert!(!MatchKind::Token.boundary_between(Some('1'), Some('.')));
        assert!(!MatchKind::Token.boundary_between(Some('1'), Some('-')));
        assert!(MatchKind::Token.boundary_between(Some('"'), Some('g')));
    }

    #[test]
    fn match_kind_round_trips() {
        for k in [MatchKind::Substring, MatchKind::Word, MatchKind::Token] {
            assert_eq!(MatchKind::parse(k.as_str()), Some(k));
        }
        assert_eq!(MatchKind::parse("Word"), None);
    }
}
