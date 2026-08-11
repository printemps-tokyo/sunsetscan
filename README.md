# sunsetscan

Scan a codebase for calls into APIs that already have an announced shutdown
date. Report every hit with `file:line`, the days remaining, and the official
migration path. Offline Rust CLI, no network access, no telemetry.

## Why

On **2026-08-26** the OpenAI Assistants API stops answering. The official
migration guide explains the mapping -- assistants become prompts, threads
become conversations, runs become responses -- and then says plainly:

> We will not provide an automated tool for migrating Threads to Conversations.

There is no codemod coming. So the first question is not "how do I migrate" but
"where, across all of this, do we still call it?" That is a code-search problem,
and it is the same problem for every dated shutdown: Google's Content API for
Shopping goes away 2026-08-18, a run of OpenAI model ids goes away through the
autumn, and each one is a different set of strings to grep for against a
different date.

sunsetscan is that grep, with the dates and the migration paths attached, and an
exit code you can put in CI.

```
$ sunsetscan .
sunsetscan: support-service
as of 2026-08-11  |  <=30d  |  18 finding(s) in 4 file(s), 5 of 10 catalog entr(ies) matched
nearest deadline: 2026-08-18  Content API for Shopping (7 days remaining)

[<=30d   ] openai-assistants-api  Assistants API
  vendor       OpenAI
  sunset       2026-08-26 (15 days remaining)
  replacement  Responses API and Conversations API
  migration    Assistants become prompt objects that hold the model, tools and instructions;
               threads become conversations; runs become responses; run steps become items.
               [...] OpenAI states it will not provide an automated tool for migrating
               threads to conversations, so every call site has to be found and rewritten
               by hand.
  guide        https://developers.openai.com/api/docs/assistants/migration
  source       https://developers.openai.com/api/docs/deprecations (verified 2026-08-11)
  13 finding(s) in 3 file(s)
  app/support_bot.py
    8:20  ASSISTANT = client.beta.assistants.create(
      matched beta.assistants (SDK property path (client.beta.assistants))
    23:9  thread_id=thread_id, assistant_id=ASSISTANT.id
      matched thread_id (run parameter (runs.create(thread_id=...)))
      matched assistant_id (run parameter (runs.create(assistant_id=...)))
  web/threads.ts
    4:35  const res = await fetch(`${BASE}/v1/threads`, { method: "POST" });
      matched /v1/threads (REST endpoint)
```

## The honest limitation

sunsetscan does not parse your code. It matches literal strings, line by line,
with an optional boundary rule. A hit inside a comment, a docstring, a changelog
or a vendored copy of somebody else's SDK is reported like any other hit.

That is a deliberate trade. A textual matcher works on every language in the
repository at once -- Python, TypeScript, Terraform, a GitHub Actions workflow,
a `.env` file -- and a parser per ecosystem would cover fewer real call sites,
not more. The tools for dealing with the resulting noise are `--exclude` and the
`sunsetscan:ignore` marker, both documented below.

Two more consequences worth stating up front:

- **Matching is case-sensitive.** Model ids and SDK paths are lowercase in
  practice, and case-insensitive matching would fire on English prose.
- **A textual match cannot tell you the call is live.** A `thread_id` column in
  a runbook and a `thread_id` argument in a run creation both match. sunsetscan
  gives you the shortlist; you decide.

## Install

```bash
cargo install --git https://github.com/printemps-tokyo/sunsetscan
```

Or build from a checkout:

```bash
git clone https://github.com/printemps-tokyo/sunsetscan
cd sunsetscan
cargo build --release   # ./target/release/sunsetscan
```

Release binaries for Linux and macOS are attached to each tagged release, with
a SHA-256 checksum next to every archive.

## Quickstart

```bash
sunsetscan                       # scan the current directory
sunsetscan path/to/repo          # scan somewhere else
sunsetscan --list                # what is in the catalog, and how long is left
sunsetscan --json | jq .         # machine-readable
sunsetscan --quiet               # one pipeable summary line

# CI: fail the build once anything is within 60 days of shutdown
sunsetscan . --fail-within 60 --as-of "$(date -u +%F)"
```

## Usage

```
sunsetscan [PATH]

  --catalog <FILE>        Merge entries from a catalog JSON file (repeatable)
  --list                  Print the catalog with dates and days remaining, then exit
  --only <ENTRY_ID>       Search only this catalog entry (repeatable)
  --skip <ENTRY_ID>       Skip this catalog entry (repeatable)
  --fail-within <DAYS>    Fail when a finding's sunset is DAYS away or fewer
  --strict                Fail on any finding at all, however distant
  --as-of <YYYY-MM-DD>    Compute days remaining from this date instead of today
  --exclude <GLOB>        Skip paths matching this gitignore-syntax glob (repeatable)
  --all-files             Scan every text file, not just known source extensions
  --json                  Emit the machine-readable document
  --no-color              Never emit ANSI color codes
  -q, --quiet             Print only the one-line summary
  -h, --help              Print help
  -V, --version           Print version
```

### Exit codes

| Code | Meaning |
| --- | --- |
| `0` | No finding breached the threshold. Findings may still have been printed. |
| `1` | At least one finding breached the threshold. |
| `2` | Usage error or I/O error (bad flag, bad date, unreadable catalog, missing directory). |

The threshold, precisely:

- Default: fail if any finding's `days_remaining <= 30`. Dates that have already
  passed are negative, so past-due findings always breach.
- `--fail-within N`: same rule with `N` instead of 30, **inclusive** -- a finding
  exactly `N` days out fails. `--fail-within 0` fails only on today's and
  past-due sunsets.
- `--strict`: fail if there is any finding at all. Mutually exclusive with
  `--fail-within`.
- Findings beyond the threshold are still reported in full; only the exit code
  changes.

### `--as-of` and reproducible CI

Days remaining are computed from today's **UTC** date by default. Resolving a
local time zone is not possible from the Rust standard library alone, and a
date crate would buy at most a one-day correction to a horizon measured in
weeks, so sunsetscan uses UTC and says so rather than pretending.

Pass `--as-of YYYY-MM-DD` to pin the reference point. Every test in this
repository does, and CI should too if you want byte-identical output:

```bash
sunsetscan . --as-of "$(date -u +%F)" --json > sunsets.json
```

### What gets scanned

The walk is gitignore-aware: `.gitignore`, `.ignore`, `.git/info/exclude`, your
global gitignore, and a sunsetscan-specific `.sunsetscanignore` are all honored,
and `.git/` is never entered. Dotfile directories such as `.github/` *are*
scanned, because workflow files name models.

Ignore files *above* the scanned directory are consulted only when that
directory is not itself a repository root. A `.gitignore` sitting above a
checkout does not apply to the checkout, and a workspace directory holding a
bare `*` would otherwise make every repository inside it scan as empty.

Skipped by default:

- Files whose extension is not in the source list (`--all-files` widens this).
  The list is broad on purpose -- `.py .ts .go .rs .tf .yml .json .md .env`,
  `Dockerfile`, `Makefile` and about seventy more. See `SOURCE_EXTENSIONS` in
  `src/scan.rs`.
- Files larger than 2 MiB.
- Files containing a NUL byte in their first 8 KiB, or that are not valid UTF-8.

Every skip is counted and reported in the footer, so nothing vanishes silently.

### Suppressing a line

Put `sunsetscan:ignore` in a trailing comment. The comment syntax does not
matter -- sunsetscan looks for the marker text anywhere on the line.

```python
legacy_thread_id = row["thread_id"]  # sunsetscan:ignore
```

Name entry ids after the marker to suppress only those:

```typescript
export const VOICE_MODEL = "gpt-4o-audio";  // sunsetscan:ignore openai-legacy-audio-realtime
```

Ids may be separated by commas or spaces. If anything after the marker is *not*
a known entry id -- prose, a ticket number, a typo -- the marker degrades to the
bare form and suppresses the whole line, which is what a reader of the comment
would expect it to do.

## Built-in catalog

Every entry below was read from the vendor's own page on **2026-08-11**. Nothing
is in the catalog that was not confirmed there.

| Sunset | Entry id | What goes away | Replacement | Source |
| --- | --- | --- | --- | --- |
| 2026-08-10 | `openai-chat-latest-aliases` | `gpt-5.2-chat-latest`, `gpt-5.3-chat-latest` | `gpt-5.6-sol` | [OpenAI deprecations](https://developers.openai.com/api/docs/deprecations) |
| 2026-08-18 | `google-content-api-shopping` | Content API for Shopping (`shoppingcontent.googleapis.com`, `/content/v2.1/`) | Merchant API | [Google migration overview](https://developers.google.com/merchant/api/guides/compatibility/overview) |
| 2026-08-26 | `openai-assistants-api` | Assistants API: `/v1/assistants`, `/v1/threads`, `client.beta.assistants`, `client.beta.threads` | Responses API and Conversations API | [OpenAI deprecations](https://developers.openai.com/api/docs/deprecations) / [migration guide](https://developers.openai.com/api/docs/assistants/migration) |
| 2026-09-24 | `openai-videos-api` | Videos API, `sora-2` aliases and snapshots | none announced | [OpenAI deprecations](https://developers.openai.com/api/docs/deprecations) |
| 2026-09-28 | `openai-legacy-completion-models` | `gpt-3.5-turbo-instruct`, `gpt-3.5-turbo-1106`, `babbage-002`, `davinci-002` | `gpt-5.6-terra` | [OpenAI deprecations](https://developers.openai.com/api/docs/deprecations) |
| 2026-10-23 | `openai-legacy-chat-models` | `gpt-4-turbo`, `gpt-4-0613`, `gpt-4o-2024-05-13`, `gpt-image-1`, `o1-*`, `o3-mini-*`, `o4-mini-*` and more | `gpt-5.6-*` / `gpt-image-2` | [OpenAI deprecations](https://developers.openai.com/api/docs/deprecations) |
| 2026-11-30 | `openai-prompts-api` | `v1/prompts` API and reusable prompt objects | prompt content in application code | [OpenAI deprecations](https://developers.openai.com/api/docs/deprecations) |
| 2026-12-01 | `openai-gpt-image-legacy` | `gpt-image-1-mini`, `gpt-image-1.5`, `chatgpt-image-latest` | `gpt-image-2` | [OpenAI deprecations](https://developers.openai.com/api/docs/deprecations) |
| 2026-12-11 | `openai-gpt5-snapshots` | `gpt-5-2025-08-07`, `gpt-5-mini/nano/pro` snapshots, `o3-2025-04-16`, `o3-pro-2025-06-10` | `gpt-5.6-sol` / `terra` / `luna` | [OpenAI deprecations](https://developers.openai.com/api/docs/deprecations) |
| 2027-01-20 | `openai-legacy-audio-realtime` | `gpt-realtime`, `gpt-audio`, `gpt-4o-realtime`, `gpt-4o-audio`, mini variants, `gpt-4o-mini-transcribe-2025-03-20` | `gpt-realtime-2.1`, `gpt-audio-1.5`, `gpt-4o-mini-transcribe-2025-12-15` | [OpenAI deprecations](https://developers.openai.com/api/docs/deprecations) |

`sunsetscan --list` prints the same table with days remaining and the full
pattern set for each entry.

**Vendors move dates.** The report footer and `--list` both print the
verification date and tell you to confirm against the `source_url`. Treat the
catalog as a shortlist, not as an oracle.

## Custom catalogs

`--catalog <file>` merges a JSON document into the built-in catalog. An entry
whose `id` already exists replaces the built-in one wholesale; a new `id` is
added. The flag is repeatable and applied in order. The catalog's displayed
verification date becomes the *oldest* of everything merged, so a stale overlay
makes the report say so.

```json
{
  "schema": 1,
  "verified_on": "2026-08-11",
  "entries": [
    {
      "id": "acme-widget-api",
      "vendor": "Acme",
      "name": "Widget API v1",
      "sunset": "2026-11-01",
      "source_url": "https://acme.example/docs/deprecations",
      "verified_on": "2026-08-11",
      "replacement": "Widget API v2",
      "migration": "widgets.v1.create() becomes widgets.v2.items.create().",
      "migration_url": "https://acme.example/docs/widget-v2-migration",
      "notes": "Announced 2026-02-01 with nine months of notice.",
      "patterns": [
        { "text": "/widgets/v1/", "kind": "substring", "note": "REST prefix" },
        { "text": "widget_api_key", "kind": "word", "note": "config key" },
        { "text": "acme-widget-1", "kind": "token", "note": "model id",
          "scope": ["**/*.py", "config/*.yml"] }
      ]
    }
  ]
}
```

### Schema

Top level:

| Field | Required | Type | Meaning |
| --- | --- | --- | --- |
| `schema` | yes | number | Must be `1`. |
| `verified_on` | yes | `YYYY-MM-DD` | When this file's contents were last checked. |
| `entries` | yes | array | The entries. |

Each entry:

| Field | Required | Type | Meaning |
| --- | --- | --- | --- |
| `id` | yes | string, `[a-z0-9-]` | Stable id; the `--only` / `--skip` and `sunsetscan:ignore` value. |
| `vendor` | yes | string | Who is shutting it down. |
| `name` | yes | string | Human-readable product name. |
| `sunset` | yes | `YYYY-MM-DD` | The announced shutdown date. |
| `source_url` | yes | http(s) URL | The vendor page the date came from. |
| `verified_on` | yes | `YYYY-MM-DD` | When that page was read. |
| `replacement` | yes | string | What the vendor says to use instead. |
| `migration` | yes | string | How to get there, summarised. |
| `migration_url` | no | http(s) URL or `null` | The vendor's migration guide. |
| `notes` | no | string or `null` | Notice periods, grace periods, caveats. |
| `patterns` | yes | array, non-empty | What to look for. |

Each pattern:

| Field | Required | Type | Meaning |
| --- | --- | --- | --- |
| `text` | yes | string, single line | The literal to search for. Case-sensitive. |
| `kind` | yes | `substring` / `word` / `token` | Match semantics; see below. |
| `note` | no | string or `null` | Printed next to the hit ("REST endpoint"). |
| `scope` | no | array of globs | gitignore-syntax globs against the repo-relative path. Empty or absent means every scanned file. |

Unknown fields are rejected, so a typo is loud rather than silent. The parser
accepts `//` and `/* */` comments and one trailing comma, so a hand-maintained
catalog can be annotated.

### Match semantics

The three kinds exist because the three things worth searching for behave
differently.

| Kind | Anchoring | Token characters | Use for |
| --- | --- | --- | --- |
| `substring` | none | -- | REST paths and SDK property paths: `/v1/threads`, `beta.assistants` |
| `word` | both ends | `[A-Za-z0-9_]` | Identifiers: `assistant_id`, `thread_id` |
| `token` | both ends | `[A-Za-z0-9_.-]` | Model ids: `gpt-image-1`, `sora-2` |

Anchoring uses the ordinary `\b` rule specialised to the given character class:
a match is accepted only if, at each end, the neighbouring character and the
pattern's own character are not *both* token characters.

The consequences are exactly the point:

- `word` -- `assistant_id` matches `runs.create(assistant_id=x)` and
  `x = assistant_id`, but **not** `my_assistant_identifier` and **not**
  `assistant_ids`.
- `token` -- `gpt-image-1` matches `"gpt-image-1"`, but **not** `gpt-image-1.5`
  and **not** `gpt-image-1-mini`. Those are different models with different
  shutdown dates and they get their own entries. The flip side is that a dated
  snapshot the vendor did not list (`gpt-4-turbo-2024-04-09`) will not match the
  alias entry (`gpt-4-turbo`); the catalog encodes only what the vendor's table
  actually names.
- `substring` -- `beta.threads` matches `client.beta.threads.runs.create(...)`
  and also matches inside a longer word, which is what you want for a property
  path and what you do not want for an identifier.

## JSON output

`--json` emits a single object. The shape is stable within a schema version:

```json
{
  "tool": "sunsetscan",
  "version": "0.1.0",
  "schema": 1,
  "root": "support-service",
  "as_of": "2026-08-11",
  "catalog_verified_on": "2026-08-11",
  "entries_searched": 10,
  "totals": { "findings": 18, "files_with_findings": 4, "entries_matched": 5 },
  "stats": { "scanned": 4, "skipped_extension": 1, "skipped_large": 0,
             "skipped_binary": 0, "skipped_unreadable": 0 },
  "nearest_deadline": { "entry_id": "google-content-api-shopping",
                        "sunset": "2026-08-18", "days_remaining": 7 },
  "findings": [
    {
      "entry": {
        "id": "openai-assistants-api", "vendor": "OpenAI", "name": "Assistants API",
        "sunset": "2026-08-26", "days_remaining": 15, "urgency": "critical",
        "source_url": "...", "verified_on": "2026-08-11",
        "replacement": "...", "migration": "...", "migration_url": "...", "notes": "..."
      },
      "count": 13,
      "files": [
        { "path": "app/support_bot.py", "count": 10,
          "lines": [
            { "line": 8, "column": 20, "snippet": "ASSISTANT = client.beta.assistants.create(",
              "patterns": [ { "text": "beta.assistants", "kind": "substring",
                              "note": "SDK property path (client.beta.assistants)",
                              "column": 20 } ] }
          ] }
      ]
    }
  ],
  "caveat": "..."
}
```

`urgency` is one of `past_due`, `critical` (0-30 days), `warning` (31-90 days),
`later`. `--list --json` emits the catalog itself in a similar shape.

Ordering is deterministic everywhere: entries by sunset date then id, files by
path, lines by line then column, patterns by column then text. The same inputs
produce byte-identical output on every machine.

## CI

```yaml
- name: Check for sunsetting APIs
  run: |
    sunsetscan . --fail-within 60 --as-of "$(date -u +%F)" --no-color
```

Exit code 1 fails the step. Add `--exclude` for vendored trees, or drop a
`.sunsetscanignore` in the repository root.

## Dependencies

`anyhow`, `clap` and `ignore`. That is the whole list, and it is meant to stay
short. JSON reading and writing, the calendar arithmetic, the word-boundary
matcher and the text wrapper are all local code, each a few dozen lines, because
each alternative would have pulled in a larger tree than it saved.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Adding a catalog entry means adding a
`source_url` and a `verified_on`; entries without a vendor page behind them are
not accepted.

## License

MIT. See [LICENSE](LICENSE).
