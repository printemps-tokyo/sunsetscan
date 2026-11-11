//! Black-box tests of the binary: flags, exit codes, JSON shape, determinism.
//!
//! Every run pins `--as-of`, so nothing here depends on the day it is run.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

/// Path to the binary under test, as cargo built it.
fn bin() -> PathBuf {
    let mut p = std::env::current_exe().expect("test exe path");
    p.pop(); // the deps/ directory
    if p.ends_with("deps") {
        p.pop();
    }
    p.join(format!("sunsetscan{}", std::env::consts::EXE_SUFFIX))
}

fn fixture(files: &[(&str, &str)]) -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    for (path, contents) in files {
        let full = dir.path().join(path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).expect("mkdir");
        }
        fs::write(&full, contents).expect("write");
    }
    dir
}

fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(bin())
        .arg(dir)
        .args(args)
        .env_remove("NO_COLOR")
        .output()
        .expect("run sunsetscan")
}

/// Run without a path argument (for `--list` and usage errors).
fn run_bare(args: &[&str]) -> Output {
    Command::new(bin())
        .args(args)
        .env_remove("NO_COLOR")
        .output()
        .expect("run sunsetscan")
}

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).expect("utf-8 stdout")
}

fn stderr(out: &Output) -> String {
    String::from_utf8(out.stderr.clone()).expect("utf-8 stderr")
}

fn code(out: &Output) -> i32 {
    out.status.code().expect("exit code")
}

/// The pinned "today" for these tests: the day the catalog was first verified.
/// The Assistants API sunsets 2026-08-26, so this is 15 days out. The catalog
/// itself was last re-verified on 2026-11-11.
const TODAY: &str = "--as-of=2026-08-11";

/// A repository that calls the Assistants API from Python and from Node.
const ASSISTANTS: [(&str, &str); 4] = [
    (
        "app/agent.py",
        "from openai import OpenAI\n\
         \n\
         client = OpenAI()\n\
         assistant = client.beta.assistants.create(\n\
         \x20   model=\"gpt-4o\",\n\
         \x20   instructions=\"Be helpful.\",\n\
         )\n\
         thread = client.beta.threads.create()\n\
         run = client.beta.threads.runs.create(\n\
         \x20   thread_id=thread.id, assistant_id=assistant.id\n\
         )\n",
    ),
    (
        "web/threads.ts",
        "const res = await fetch(\"https://api.openai.com/v1/threads\", {\n\
         \x20 method: \"POST\",\n\
         });\n",
    ),
    (
        "docs/architecture.md",
        "The worker stores an assistant_id per tenant.\n",
    ),
    (
        "app/other.py",
        "# Nothing interesting in this file.\nprint('hello')\n",
    ),
];

// ---------------------------------------------------------------------------
// The motivating case
// ---------------------------------------------------------------------------

#[test]
fn reports_assistants_api_call_sites_with_line_numbers_and_the_migration_path() {
    let dir = fixture(&ASSISTANTS);
    let out = run(dir.path(), &[TODAY, "--no-color"]);
    let text = stdout(&out);

    assert_eq!(code(&out), 1, "15 days out must fail the default threshold");
    for needle in [
        "openai-assistants-api",
        "OpenAI",
        "2026-08-26 (15 days remaining)",
        "Responses API and Conversations API",
        "https://developers.openai.com/api/docs/assistants/migration",
        "https://developers.openai.com/api/docs/deprecations (verified 2026-11-11)",
        "will not provide an automated tool",
        "app/agent.py",
        "web/threads.ts",
        "docs/architecture.md",
        "matched beta.assistants",
        "matched /v1/threads",
        "matched assistant_id",
        "nearest deadline: 2026-08-26",
        "Catalog verified as of 2026-11-11.",
        "vendors move dates",
    ] {
        assert!(text.contains(needle), "missing {needle:?} in:\n{text}");
    }
    // The clean file is not mentioned.
    assert!(!text.contains("app/other.py"), "{text}");
}

#[test]
fn a_comment_only_mention_is_reported_because_matching_is_textual() {
    // This documents the limitation rather than papering over it: the markdown
    // file only talks about assistant_id, and sunsetscan still reports it.
    let dir = fixture(&[("docs/architecture.md", "We store an assistant_id.\n")]);
    let out = run(dir.path(), &[TODAY, "--no-color"]);
    assert_eq!(code(&out), 1);
    assert!(stdout(&out).contains("docs/architecture.md"));
    assert!(stdout(&out).contains("matched assistant_id"));
}

#[test]
fn a_clean_repository_exits_zero_and_says_nothing_was_found() {
    let dir = fixture(&[
        (
            "app.py",
            "client.responses.create(model=\"gpt-5.6-sol\", input=[])\n",
        ),
        ("web/a.ts", "await client.conversations.create();\n"),
    ]);
    let out = run(dir.path(), &[TODAY, "--no-color"]);
    assert_eq!(code(&out), 0, "{}", stdout(&out));
    assert!(stdout(&out).contains("No calls into a sunsetting API were found."));
    assert!(stdout(&out).contains("Catalog verified as of"));
}

// ---------------------------------------------------------------------------
// Word-boundary semantics
// ---------------------------------------------------------------------------

#[test]
fn a_longer_identifier_does_not_match_a_word_pattern() {
    let dir = fixture(&[(
        "app.py",
        "my_assistant_identifier = 1\nother_assistant_ids = []\n",
    )]);
    let out = run(dir.path(), &[TODAY, "--no-color"]);
    assert_eq!(code(&out), 0, "{}", stdout(&out));
    assert!(stdout(&out).contains("No calls into a sunsetting API were found."));
}

#[test]
fn a_neighbouring_model_id_does_not_match_a_token_pattern() {
    // gpt-image-1 sunsets 2026-10-23; gpt-image-1.5 sunsets 2026-12-01. Each
    // must be attributed to its own entry.
    let dir = fixture(&[("a.py", "M = \"gpt-image-1.5\"\n")]);
    let text = stdout(&run(dir.path(), &[TODAY, "--no-color"]));
    assert!(text.contains("openai-gpt-image-legacy"), "{text}");
    assert!(!text.contains("openai-legacy-chat-models"), "{text}");

    let dir = fixture(&[("a.py", "M = \"gpt-image-1\"\n")]);
    let text = stdout(&run(dir.path(), &[TODAY, "--no-color"]));
    assert!(text.contains("openai-legacy-chat-models"), "{text}");
    assert!(!text.contains("openai-gpt-image-legacy"), "{text}");
}

#[test]
fn gemini_model_ids_are_attributed_to_their_own_entries() {
    // gemini-2.5-flash is rerouted 2026-12-01, gemini-2.5-flash-image shuts
    // down 2027-03-15, and gemini-2.5-flash-lite has no announced date.
    let dir = fixture(&[("a.py", "M = \"models/gemini-2.5-flash\"\n")]);
    let out = run(dir.path(), &["--as-of=2026-11-11", "--no-color"]);
    let text = stdout(&out);
    assert_eq!(code(&out), 1, "20 days out must fail the default threshold");
    assert!(text.contains("google-gemini-2-5-flash "), "{text}");
    assert!(text.contains("2026-12-01 (20 days remaining)"), "{text}");
    assert!(!text.contains("google-gemini-2-5-flash-image"), "{text}");

    let dir = fixture(&[("a.py", "M = \"gemini-2.5-flash-image\"\n")]);
    let out = run(dir.path(), &["--as-of=2026-11-11", "--no-color"]);
    let text = stdout(&out);
    assert_eq!(code(&out), 0, "{text}");
    assert!(text.contains("google-gemini-2-5-flash-image"), "{text}");
    assert!(text.contains("2027-03-15"), "{text}");

    let dir = fixture(&[("a.py", "M = \"gemini-2.5-flash-lite\"\n")]);
    let out = run(dir.path(), &["--as-of=2026-11-11", "--no-color"]);
    assert_eq!(code(&out), 0, "{}", stdout(&out));
    assert!(stdout(&out).contains("No calls into a sunsetting API were found."));
}

#[test]
fn the_claude_sonnet_4_5_snapshot_matches_but_other_claude_ids_do_not() {
    let dir = fixture(&[("a.ts", "const MODEL = \"claude-sonnet-4-5-20250929\";\n")]);
    let out = run(dir.path(), &["--as-of=2026-11-11", "--no-color"]);
    let text = stdout(&out);
    assert_eq!(code(&out), 1, "{text}");
    for needle in [
        "anthropic-claude-sonnet-4-5",
        "2026-11-30 (19 days remaining)",
        "claude-sonnet-5-5",
        "https://platform.claude.com/docs/en/about-claude/model-deprecations",
    ] {
        assert!(text.contains(needle), "missing {needle:?} in:\n{text}");
    }

    // A different model, and a partner-platform id that embeds the snapshot:
    // the vendor page gives neither of them this date.
    let dir = fixture(&[(
        "a.ts",
        "const A = \"claude-sonnet-4-6\";\n\
         const B = \"anthropic.claude-sonnet-4-5-20250929-v1:0\";\n",
    )]);
    let out = run(dir.path(), &["--as-of=2026-11-11", "--no-color"]);
    assert_eq!(code(&out), 0, "{}", stdout(&out));
}

#[test]
fn newer_openai_entries_separate_aliases_snapshots_and_endpoints() {
    // gpt-image-2 (2027-02-10) must not swallow its 2.6 replacements, and the
    // audio translations endpoint is matched as a path.
    let dir = fixture(&[(
        "a.py",
        "IMG = \"gpt-image-2\"\n\
         NEW = \"gpt-image-2.6-flare\"\n\
         URL = \"https://api.openai.com/v1/audio/translations\"\n\
         TTS = \"tts-1-hd\"\n\
         OLD = \"gpt-4o-2024-08-06\"\n\
         KEEP = \"gpt-4o\"\n",
    )]);
    let out = run(dir.path(), &["--as-of=2026-11-11", "--no-color"]);
    let text = stdout(&out);
    assert_eq!(code(&out), 0, "nothing here is within 30 days:\n{text}");
    for needle in [
        "openai-gpt-image-2",
        "openai-transcription-models",
        "matched /v1/audio/translations",
        "openai-tts-models",
        "matched tts-1-hd",
        "openai-gpt4-series-snapshots",
        "4 finding(s) in 1 file(s), 4 of 24 catalog entr(ies) matched",
    ] {
        assert!(text.contains(needle), "missing {needle:?} in:\n{text}");
    }
    assert!(!text.contains("matched tts-1 "), "{text}");
    assert_eq!(
        code(&run(dir.path(), &["--as-of=2026-11-11", "--strict"])),
        1
    );
}

// ---------------------------------------------------------------------------
// Suppression and exclusion
// ---------------------------------------------------------------------------

#[test]
fn the_inline_marker_suppresses_a_line() {
    let dir = fixture(&[(
        "app.py",
        "thread = client.beta.threads.create()  # sunsetscan:ignore\n",
    )]);
    let out = run(dir.path(), &[TODAY, "--no-color"]);
    assert_eq!(code(&out), 0, "{}", stdout(&out));
}

#[test]
fn the_marker_can_name_the_entry_it_suppresses() {
    let line = "M = \"gpt-4-turbo\"  # sunsetscan:ignore openai-videos-api\n";
    let dir = fixture(&[("app.py", line)]);
    // Suppressing an unrelated entry leaves the finding in place.
    assert!(stdout(&run(dir.path(), &[TODAY, "--no-color"])).contains("openai-legacy-chat-models"));

    let dir = fixture(&[(
        "app.py",
        "M = \"gpt-4-turbo\"  # sunsetscan:ignore openai-legacy-chat-models\n",
    )]);
    let out = run(dir.path(), &[TODAY, "--no-color"]);
    assert_eq!(code(&out), 0);
    assert!(stdout(&out).contains("No calls into a sunsetting API were found."));
}

#[test]
fn exclude_globs_drop_whole_paths() {
    let dir = fixture(&ASSISTANTS);
    let text = stdout(&run(
        dir.path(),
        &[
            TODAY,
            "--no-color",
            "--exclude",
            "docs/**",
            "--exclude",
            "web/**",
        ],
    ));
    assert!(text.contains("app/agent.py"), "{text}");
    assert!(!text.contains("docs/architecture.md"), "{text}");
    assert!(!text.contains("web/threads.ts"), "{text}");
}

#[test]
fn gitignored_files_are_invisible_to_the_cli_too() {
    let dir = fixture(&[
        (".gitignore", "vendor/\n"),
        ("app.py", "print('clean')\n"),
        ("vendor/sdk.py", "client.beta.threads.create()\n"),
    ]);
    assert_eq!(code(&run(dir.path(), &[TODAY, "--no-color"])), 0);
}

// ---------------------------------------------------------------------------
// --as-of, thresholds and exit codes
// ---------------------------------------------------------------------------

#[test]
fn as_of_drives_days_remaining_and_the_severity_bucket() {
    let dir = fixture(&[("app.py", "client.beta.threads.create()\n")]);

    let text = stdout(&run(dir.path(), &["--as-of=2026-01-01", "--no-color"]));
    assert!(text.contains("[later"), "{text}");
    assert!(text.contains("237 days remaining"), "{text}");

    let text = stdout(&run(dir.path(), &["--as-of=2026-06-01", "--no-color"]));
    assert!(text.contains("[<=90d"), "{text}");
    assert!(text.contains("86 days remaining"), "{text}");

    let text = stdout(&run(dir.path(), &["--as-of=2026-08-11", "--no-color"]));
    assert!(text.contains("[<=30d"), "{text}");
    assert!(text.contains("15 days remaining"), "{text}");

    let text = stdout(&run(dir.path(), &["--as-of=2026-08-26", "--no-color"]));
    assert!(text.contains("[<=30d"), "{text}");
    assert!(text.contains("2026-08-26 (today)"), "{text}");

    let text = stdout(&run(dir.path(), &["--as-of=2026-09-15", "--no-color"]));
    assert!(text.contains("[PAST DUE"), "{text}");
    assert!(text.contains("20 days ago"), "{text}");
}

#[test]
fn a_past_due_finding_fails_even_with_fail_within_zero() {
    let dir = fixture(&[("app.py", "client.beta.threads.create()\n")]);
    let out = run(
        dir.path(),
        &["--as-of=2026-09-15", "--no-color", "--fail-within", "0"],
    );
    assert_eq!(code(&out), 1);
    assert!(stdout(&out).contains("PAST DUE"));
}

#[test]
fn fail_within_is_inclusive_at_the_boundary() {
    let dir = fixture(&[("app.py", "client.beta.threads.create()\n")]);
    // 15 days remaining as of 2026-08-11.
    assert_eq!(code(&run(dir.path(), &[TODAY, "--fail-within", "14"])), 0);
    assert_eq!(code(&run(dir.path(), &[TODAY, "--fail-within", "15"])), 1);
    assert_eq!(code(&run(dir.path(), &[TODAY, "--fail-within", "16"])), 1);
}

#[test]
fn a_distant_finding_passes_by_default_and_fails_under_strict() {
    // The GPT-5 snapshots sunset 2026-12-11, 122 days after the pinned date.
    let dir = fixture(&[("app.py", "MODEL = \"gpt-5-2025-08-07\"\n")]);
    let out = run(dir.path(), &[TODAY, "--no-color"]);
    assert_eq!(code(&out), 0, "{}", stdout(&out));
    assert!(stdout(&out).contains("openai-gpt5-snapshots"));
    assert!(stdout(&out).contains("122 days remaining"));

    assert_eq!(code(&run(dir.path(), &[TODAY, "--strict"])), 1);
}

#[test]
fn strict_on_a_clean_repository_still_exits_zero() {
    let dir = fixture(&[("app.py", "print('clean')\n")]);
    assert_eq!(code(&run(dir.path(), &[TODAY, "--strict"])), 0);
}

#[test]
fn strict_and_fail_within_cannot_be_combined() {
    let dir = fixture(&[("app.py", "print('clean')\n")]);
    let out = run(dir.path(), &[TODAY, "--strict", "--fail-within", "5"]);
    assert_eq!(code(&out), 2);
    assert!(
        stderr(&out).contains("cannot be used with"),
        "{}",
        stderr(&out)
    );
}

// ---------------------------------------------------------------------------
// Catalog selection
// ---------------------------------------------------------------------------

#[test]
fn only_and_skip_narrow_the_catalog() {
    let dir = fixture(&[(
        "app.py",
        "client.beta.threads.create()\nMODEL = \"gpt-4-turbo\"\n",
    )]);
    let both = stdout(&run(dir.path(), &[TODAY, "--no-color"]));
    assert!(both.contains("openai-assistants-api"));
    assert!(both.contains("openai-legacy-chat-models"));

    let only = stdout(&run(
        dir.path(),
        &[TODAY, "--no-color", "--only", "openai-assistants-api"],
    ));
    assert!(only.contains("openai-assistants-api"));
    assert!(!only.contains("openai-legacy-chat-models"));
    assert!(only.contains("1 of 1 catalog entr(ies) matched"), "{only}");

    let skipped = stdout(&run(
        dir.path(),
        &[TODAY, "--no-color", "--skip", "openai-assistants-api"],
    ));
    assert!(!skipped.contains("[<=30d] openai-assistants-api"));
    assert!(skipped.contains("openai-legacy-chat-models"));
}

#[test]
fn an_unknown_entry_id_is_a_usage_error() {
    let dir = fixture(&[("app.py", "print('x')\n")]);
    let out = run(dir.path(), &[TODAY, "--only", "no-such-entry"]);
    assert_eq!(code(&out), 2);
    let err = stderr(&out);
    assert!(
        err.contains("unknown catalog entry id 'no-such-entry'"),
        "{err}"
    );
    assert!(err.contains("openai-assistants-api"), "{err}");
}

#[test]
fn a_custom_catalog_adds_and_overrides_entries() {
    let dir = fixture(&[(
        "app.py",
        "legacy_widget_api(payload)\nclient.beta.threads.create()\n",
    )]);
    let cat = dir.path().join("extra.json");
    fs::write(
        &cat,
        r#"{
          "schema": 1,
          "verified_on": "2026-07-01",
          "entries": [
            {
              "id": "acme-widget-api",
              "vendor": "Acme",
              "name": "Widget API v1",
              "sunset": "2026-08-20",
              "source_url": "https://acme.example/deprecations",
              "verified_on": "2026-07-01",
              "replacement": "Widget API v2",
              "migration": "Call widgets.v2 instead.",
              "patterns": [{"text": "legacy_widget_api", "kind": "word", "note": "helper"}]
            },
            {
              "id": "openai-assistants-api",
              "vendor": "OpenAI",
              "name": "Assistants API (local override)",
              "sunset": "2027-01-01",
              "source_url": "https://internal.example/waiver",
              "verified_on": "2026-07-01",
              "replacement": "Responses API",
              "migration": "We have an extension until January.",
              "patterns": [{"text": "beta.threads", "kind": "substring"}]
            }
          ]
        }"#,
    )
    .unwrap();

    let out = run(
        dir.path(),
        &[
            TODAY,
            "--no-color",
            "--catalog",
            cat.to_str().unwrap(),
            "--exclude",
            "extra.json",
        ],
    );
    let text = stdout(&out);
    // The new entry fires.
    assert!(text.contains("acme-widget-api"), "{text}");
    assert!(text.contains("Widget API v1"), "{text}");
    assert!(text.contains("2026-08-20 (9 days remaining)"), "{text}");
    // The override replaced the built-in entry wholesale.
    assert!(text.contains("Assistants API (local override)"), "{text}");
    assert!(text.contains("2027-01-01"), "{text}");
    assert!(!text.contains("developers.openai.com/api/docs/assistants/migration"));
    // The stalest verified_on wins in the footer.
    assert!(
        text.contains("Catalog verified as of 2026-07-01."),
        "{text}"
    );
}

#[test]
fn a_malformed_custom_catalog_is_a_usage_error() {
    let dir = fixture(&[("app.py", "print('x')\n")]);
    let cat = dir.path().join("bad.json");
    fs::write(&cat, r#"{"schema": 1, "verified_on": "2026-07-01"}"#).unwrap();
    let out = run(dir.path(), &[TODAY, "--catalog", cat.to_str().unwrap()]);
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("entries"), "{}", stderr(&out));

    let out = run(dir.path(), &[TODAY, "--catalog", "/nope/missing.json"]);
    assert_eq!(code(&out), 2);
    assert!(
        stderr(&out).contains("cannot read catalog"),
        "{}",
        stderr(&out)
    );
}

// ---------------------------------------------------------------------------
// --list
// ---------------------------------------------------------------------------

#[test]
fn list_prints_the_catalog_with_dates_and_days_remaining() {
    let out = run_bare(&["--list", TODAY, "--no-color"]);
    assert_eq!(code(&out), 0);
    let text = stdout(&out);
    for needle in [
        "openai-assistants-api",
        "2026-08-26",
        "15 days remaining",
        "google-content-api-shopping",
        "2026-08-18",
        "https://developers.google.com/merchant/api/guides/compatibility/overview",
        "beta.threads [substring]",
        "assistant_id [word]",
        "gpt-4-turbo [token]",
        "Catalog verified as of 2026-11-11.",
        "vendors move dates",
    ] {
        assert!(text.contains(needle), "missing {needle:?} in:\n{text}");
    }
}

#[test]
fn list_respects_only_and_a_custom_catalog() {
    let out = run_bare(&["--list", TODAY, "--only", "openai-videos-api"]);
    let text = stdout(&out);
    assert!(text.contains("openai-videos-api"));
    assert!(!text.contains("openai-assistants-api"));
}

#[test]
fn list_json_is_valid_and_carries_every_entry() {
    let out = run_bare(&["--list", TODAY, "--json"]);
    assert_eq!(code(&out), 0);
    let text = stdout(&out);
    assert!(text.starts_with('{'), "{text}");
    assert!(text.contains("\"id\": \"openai-assistants-api\""), "{text}");
    assert!(text.contains("\"days_remaining\": 15"), "{text}");
    assert!(text.contains("\"urgency\": \"critical\""), "{text}");
    assert!(text.contains("\"kind\": \"token\""), "{text}");
}

// ---------------------------------------------------------------------------
// Output modes
// ---------------------------------------------------------------------------

#[test]
fn json_output_is_stable_and_has_the_documented_shape() {
    let dir = fixture(&ASSISTANTS);
    let out = run(dir.path(), &[TODAY, "--json"]);
    assert_eq!(code(&out), 1);
    let text = stdout(&out);

    for needle in [
        "\"tool\": \"sunsetscan\"",
        "\"schema\": 1",
        "\"as_of\": \"2026-08-11\"",
        "\"catalog_verified_on\": \"2026-11-11\"",
        "\"entries_searched\": 24",
        "\"nearest_deadline\": {\"entry_id\": \"openai-assistants-api\"",
        "\"days_remaining\": 15",
        "\"urgency\": \"critical\"",
        "\"path\": \"app/agent.py\"",
        "\"line\": 4,",
        "\"kind\": \"substring\"",
        "\"caveat\":",
    ] {
        assert!(text.contains(needle), "missing {needle:?} in:\n{text}");
    }
    // Never colored, even though stdout was captured with color un-suppressed.
    assert!(!text.contains('\x1b'));
    // Byte-for-byte identical across runs.
    assert_eq!(text, stdout(&run(dir.path(), &[TODAY, "--json"])));
}

#[test]
fn quiet_prints_one_line_to_stdout_and_the_caveat_to_stderr() {
    let dir = fixture(&ASSISTANTS);
    let out = run(dir.path(), &[TODAY, "--quiet", "--no-color"]);
    assert_eq!(code(&out), 1);
    assert_eq!(stdout(&out).lines().count(), 1, "{}", stdout(&out));
    assert!(stdout(&out).starts_with("sunsetscan: "));
    assert!(stderr(&out).contains("Catalog verified as of 2026-11-11"));
}

#[test]
fn output_is_never_colored_when_piped() {
    let dir = fixture(&ASSISTANTS);
    // stdout is a pipe here, so color must be off even without --no-color.
    assert!(!stdout(&run(dir.path(), &[TODAY])).contains('\x1b'));
}

#[test]
fn findings_come_out_in_a_stable_order() {
    let dir = fixture(&[
        ("z/last.py", "client.beta.threads.create()\n"),
        ("a/first.py", "client.beta.threads.create()\n"),
        ("m/mid.py", "MODEL = \"gpt-4-turbo\"\n"),
    ]);
    let text = stdout(&run(dir.path(), &[TODAY, "--no-color"]));
    // Entries ascend by sunset date: assistants (08-26) before chat models (10-23).
    let assistants = text.find("openai-assistants-api").unwrap();
    let chat = text.find("openai-legacy-chat-models").unwrap();
    assert!(assistants < chat, "{text}");
    // Files ascend by path within an entry.
    assert!(text.find("a/first.py").unwrap() < text.find("z/last.py").unwrap());
    // And the whole thing is byte-identical run to run.
    assert_eq!(text, stdout(&run(dir.path(), &[TODAY, "--no-color"])));
}

// ---------------------------------------------------------------------------
// Usage errors
// ---------------------------------------------------------------------------

#[test]
fn a_missing_directory_exits_two() {
    let out = run_bare(&["/definitely/not/here", TODAY]);
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("not a directory"), "{}", stderr(&out));
}

#[test]
fn a_malformed_as_of_exits_two() {
    let dir = fixture(&[("app.py", "print('x')\n")]);
    let out = run(dir.path(), &["--as-of=2026/08/11"]);
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("YYYY-MM-DD"), "{}", stderr(&out));
}

#[test]
fn a_negative_fail_within_exits_two() {
    let dir = fixture(&[("app.py", "print('x')\n")]);
    let out = run(dir.path(), &[TODAY, "--fail-within", "-3"]);
    assert_eq!(code(&out), 2);
}

#[test]
fn skipping_every_entry_is_a_usage_error() {
    let dir = fixture(&[("app.py", "print('x')\n")]);
    let ids: Vec<String> = sunsetscan::catalog::Catalog::builtin()
        .ids()
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut args: Vec<String> = vec![TODAY.to_string()];
    for id in &ids {
        args.push("--skip".into());
        args.push(id.clone());
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = run(dir.path(), &refs);
    assert_eq!(code(&out), 2);
    assert!(
        stderr(&out).contains("no catalog entries"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn help_and_version_work() {
    let out = run_bare(&["--help"]);
    assert_eq!(code(&out), 0);
    let text = stdout(&out);
    for needle in [
        "--fail-within",
        "--as-of",
        "--catalog",
        "--list",
        "--exclude",
        "--all-files",
        "EXIT CODES:",
        "sunsetscan:ignore",
        "textual, not semantic",
    ] {
        assert!(text.contains(needle), "missing {needle:?} in help");
    }

    let out = run_bare(&["--version"]);
    assert_eq!(code(&out), 0);
    assert!(stdout(&out).contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn all_files_widens_the_extension_filter() {
    let dir = fixture(&[("weird.unknownext", "client.beta.threads.create()\n")]);
    assert_eq!(code(&run(dir.path(), &[TODAY])), 0);
    let out = run(dir.path(), &[TODAY, "--all-files", "--no-color"]);
    assert_eq!(code(&out), 1);
    assert!(stdout(&out).contains("weird.unknownext"));
}
