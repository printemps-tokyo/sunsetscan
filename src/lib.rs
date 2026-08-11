//! sunsetscan -- find the code that calls an API with an announced shutdown
//! date, and say how long is left.
//!
//! # The problem
//!
//! On 2026-08-26 the OpenAI Assistants API stops answering. The migration guide
//! at <https://developers.openai.com/api/docs/assistants/migration> (read
//! 2026-08-11) explains the mapping -- assistants become prompts, threads
//! become conversations, runs become responses -- and then says plainly: "We
//! will not provide an automated tool for migrating Threads to Conversations."
//!
//! There is no codemod coming. Which means the first question is not "how do I
//! migrate" but "where, in all of this, do we still call it?" That is a
//! code-search problem, and it is the same problem for every dated shutdown:
//! Google's Content API for Shopping goes away 2026-08-18, a pile of OpenAI
//! model ids go away through the autumn, and each one is a different set of
//! strings to grep for and a different date to count down to.
//!
//! sunsetscan is that grep, with the dates and the migration paths attached.
//!
//! # What it is not
//!
//! It does not parse your code. Matching is literal text, line by line, so a
//! hit inside a comment, a changelog or a vendored SDK is reported like any
//! other. That is a deliberate trade: a textual matcher works on every language
//! in the repository, and the alternative -- a parser per ecosystem -- would
//! cover fewer call sites, not more. `--exclude` and the `sunsetscan:ignore`
//! marker are how you deal with the noise.
//!
//! # Accuracy
//!
//! Every catalog entry names the vendor page its date came from and the date
//! that page was read. Nothing is encoded that was not confirmed on a
//! vendor-owned page. Vendors do move dates, so both the report and `--list`
//! print the verification date and tell you to check the source.
//!
//! # Shape
//!
//! - [`catalog::Catalog`] is data, parsed from JSON (built-in or `--catalog`).
//! - [`matcher::Matcher`] is pure: a catalog plus text in, [`matcher::Hit`]s out.
//! - [`model::Report::build`] is pure: hits in, a grouped and ordered report out.
//! - [`report`] renders text or JSON.
//! - [`scan`] is the only module that touches the filesystem.
//!
//! No network access happens at any point.
//!
//! ```no_run
//! use sunsetscan::{catalog::Catalog, matcher::Matcher, model::Report, report, scan};
//!
//! let catalog = Catalog::builtin();
//! let matcher = Matcher::new(&catalog)?;
//! let (hits, stats) = scan::scan_tree(
//!     std::path::Path::new("."),
//!     &scan::ScanOptions::default(),
//!     &matcher,
//! )?;
//! let report = Report::build(
//!     "my-repo".into(),
//!     sunsetscan::date::Date::today_utc(),
//!     &catalog,
//!     hits,
//!     stats,
//! );
//! print!("{}", report::render_text(&report, report::RenderOptions::default()));
//! # Ok::<(), String>(())
//! ```

pub mod catalog;
pub mod date;
pub mod json;
pub mod matcher;
pub mod model;
pub mod report;
pub mod scan;

pub use catalog::{Catalog, Entry, MatchKind, Pattern};
pub use date::Date;
pub use matcher::{FileHits, Hit, MatchedPattern, Matcher, IGNORE_MARKER};
pub use model::{
    EntryFindings, FailPolicy, FileFindings, LineFinding, Report, ScanStats, Urgency, CAVEAT,
    CRITICAL_DAYS, WARNING_DAYS,
};
pub use report::{render_json, render_list_json, render_list_text, render_text, RenderOptions};
pub use scan::{scan_tree, ScanOptions, MAX_FILE_BYTES};
