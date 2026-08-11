# Contributing

Thanks for your interest in contributing to sunsetscan.

## Development

1. Fork / clone the repository
2. Install a stable Rust toolchain (https://rustup.rs)
3. Create a branch: `git switch -c feat/your-change`
4. Make your change and verify locally:
   ```bash
   cargo fmt --all -- --check
   cargo clippy --all-targets -- -D warnings
   cargo test --all
   cargo build --release
   ```
5. Commit and open a pull request

## Adding a catalog entry

This is the most common contribution, and it has the strictest rule.

sunsetscan tells people that somebody else's API is going away on a specific
date. That is only defensible with a citation, so **every entry must record the
vendor page the date came from and the date that page was read**. Entries whose
date came from a blog post, a forum thread, a changelog aggregator or a model's
memory are not accepted.

Add the entry to `src/catalog.json`. It is the only file that has to change; the
catalog is embedded at build time and parsed by the same validator that handles
`--catalog` files, so the schema in `README.md` is exactly what applies.

A pull request adding an entry should:

- set `source_url` to a **vendor-owned** page (`developers.openai.com`,
  `developers.google.com`, `docs.aws.amazon.com`, ...) that states the date, and
  `verified_on` to the day you read it
- quote the sentence that establishes the date in the pull request description
- leave the entry out entirely if the date cannot be confirmed, rather than
  guessing. A missing entry is a gap; a wrong date is a false alarm that costs
  somebody a sprint
- derive patterns from what the vendor's own docs and migration guide show --
  the REST paths, the SDK property paths, the model ids they actually print. Do
  not invent SDK symbol names from memory
- choose `kind` deliberately: `substring` for REST and property paths, `word`
  for identifiers, `token` for model ids. Get this wrong and a pattern either
  misses real call sites or fires on a neighbouring model with a different date
- add a test. `tests/cli.rs` has examples covering a hit, a near-miss that must
  *not* match, and the entry's effect on the exit code
- add a row to the catalog table in `README.md` in the same change

When the whole catalog is re-verified against its sources, bump the top-level
`verified_on` in `src/catalog.json` along with each entry's own.

## Updating a date

Vendors move dates. When one moves, update `sunset`, `verified_on` and, if the
wording changed, `migration` and `notes` together, in one commit, and say in the
description what the vendor page now reads.

## Detection changes

Matching semantics (`substring` / `word` / `token`) are documented in
`README.md` and pinned by tests in `src/matcher.rs`. Changing them changes what
every existing entry matches, so a pull request that touches the boundary rules
needs to say which entries it affects and add cases for the new behavior on both
sides of the boundary.

sunsetscan deliberately does **not** parse source code, and does not try to
detect whether a hit is inside a comment. That is stated plainly in the README
rather than half-solved. Proposals to add language parsing should start as an
issue, because the trade -- fewer languages covered, more code, more ways to be
subtly wrong -- is a project-level decision.

## Dependencies

sunsetscan depends on `anyhow`, `clap` and `ignore`, and that is meant to stay
short. JSON, dates, glob-free matching and text wrapping are local code on
purpose. A pull request adding a dependency needs to explain why a few dozen
lines of local code is worse. Dependency updates are handled by Dependabot (see
`.github/dependabot.yml`).

## Commit messages

[Conventional Commits](https://www.conventionalcommits.org/) are preferred
(`feat:`, `fix:`, `docs:`, `chore:`, `refactor:`, `test:`). English, no emoji.

## Pull requests

- Keep one PR focused on a single purpose
- CI (fmt / clippy / test / build / self-scan) must be green
- Update the README and tests when behavior changes

## Bug reports and requests

Please open an [issue](../../issues) using the templates. For a false positive
or a false negative, include the line that did or did not match and the entry
id -- that is usually enough to fix it.
