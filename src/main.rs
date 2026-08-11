//! Command-line entry point for sunsetscan.

use std::io::{IsTerminal, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;

use sunsetscan::catalog::Catalog;
use sunsetscan::date::Date;
use sunsetscan::matcher::Matcher;
use sunsetscan::model::{FailPolicy, Report, CRITICAL_DAYS};
use sunsetscan::report::{
    render_json, render_list_json, render_list_text, render_text, summary_line, RenderOptions,
};
use sunsetscan::scan::{scan_tree, ScanOptions};

const AFTER_HELP: &str = "\
EXIT CODES:
  0  no finding breached the threshold
  1  at least one finding did
  2  usage or I/O error

THRESHOLD:
  By default a run fails when any finding's sunset is 30 or fewer days away,
  which includes dates that have already passed. --fail-within N moves that
  boundary (inclusive: N=15 fails on a finding exactly 15 days out). --strict
  fails on any finding at all. Findings past the threshold are still reported;
  only the exit code changes.

SUPPRESSION:
  A line carrying a trailing `sunsetscan:ignore` comment is skipped. Name entry
  ids after the marker (`sunsetscan:ignore openai-videos-api, openai-prompts-api`)
  to suppress only those; anything the marker does not recognise as an entry id
  makes it suppress the whole line.

Matching is textual, not semantic. sunsetscan does not parse your code, so hits
inside comments, documentation and vendored SDKs are reported like any other.
Catalog dates come from the vendor pages named in each entry; vendors move
dates, so confirm against the source_url before acting on a finding.";

/// Scan a codebase for calls into APIs with an announced shutdown date.
#[derive(Parser, Debug)]
#[command(
    name = "sunsetscan",
    version,
    about,
    long_about = None,
    after_help = AFTER_HELP
)]
struct Cli {
    /// Directory to scan.
    #[arg(default_value = ".")]
    path: PathBuf,

    /// Merge entries from a catalog JSON file; same id replaces (repeatable).
    #[arg(long, value_name = "FILE")]
    catalog: Vec<PathBuf>,

    /// Print the catalog with sunset dates and days remaining, then exit.
    #[arg(long)]
    list: bool,

    /// Search only this catalog entry (repeatable).
    #[arg(long, value_name = "ENTRY_ID")]
    only: Vec<String>,

    /// Skip this catalog entry (repeatable).
    #[arg(long, value_name = "ENTRY_ID")]
    skip: Vec<String>,

    /// Exit non-zero when a finding's sunset is this many days away or fewer.
    #[arg(long, value_name = "DAYS", conflicts_with = "strict")]
    fail_within: Option<i64>,

    /// Exit non-zero on any finding, however distant.
    #[arg(long)]
    strict: bool,

    /// Compute days remaining from this date instead of today (UTC).
    #[arg(long, value_name = "YYYY-MM-DD", value_parser = Date::parse)]
    as_of: Option<Date>,

    /// Skip paths matching this gitignore-syntax glob (repeatable).
    #[arg(long, value_name = "GLOB")]
    exclude: Vec<String>,

    /// Scan every text file, not just known source extensions.
    #[arg(long)]
    all_files: bool,

    /// Emit the machine-readable JSON document instead of the human report.
    #[arg(long)]
    json: bool,

    /// Never emit ANSI color codes.
    #[arg(long)]
    no_color: bool,

    /// Print only the one-line summary.
    #[arg(short, long)]
    quiet: bool,
}

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(err) => {
            eprintln!("sunsetscan: {err:#}");
            std::process::exit(2);
        }
    }
}

fn run() -> Result<i32> {
    let cli = Cli::parse();

    if let Some(days) = cli.fail_within {
        if days < 0 {
            anyhow::bail!("--fail-within must not be negative");
        }
    }

    let mut catalog = Catalog::builtin();
    for path in &cli.catalog {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read catalog {}", path.display()))?;
        let overlay = Catalog::from_json(&text, &path.display().to_string())
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        catalog.merge(overlay);
    }
    catalog
        .retain(&cli.only, &cli.skip)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let as_of = cli.as_of.unwrap_or_else(Date::today_utc);

    // Color is opt-out via the flag, the NO_COLOR convention, or a non-tty
    // stdout; JSON is never colored.
    let color = !cli.no_color
        && !cli.json
        && std::env::var_os("NO_COLOR").is_none()
        && std::io::stdout().is_terminal();
    let opts = RenderOptions { color };

    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    if cli.list {
        if cli.json {
            out.write_all(render_list_json(&catalog, as_of).as_bytes())?;
        } else {
            out.write_all(render_list_text(&catalog, as_of, opts).as_bytes())?;
        }
        out.flush()?;
        return Ok(0);
    }

    if !cli.path.is_dir() {
        anyhow::bail!("not a directory: {}", cli.path.display());
    }
    if catalog.is_empty() {
        anyhow::bail!("--only / --skip left no catalog entries to search for");
    }

    let root_name = cli
        .path
        .canonicalize()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| cli.path.display().to_string());

    let matcher = Matcher::new(&catalog).map_err(|e| anyhow::anyhow!("{e}"))?;
    let scan_opts = ScanOptions {
        all_files: cli.all_files,
        exclude: cli.exclude.clone(),
    };
    let (hits, stats) = scan_tree(&cli.path, &scan_opts, &matcher)
        .map_err(|e| anyhow::anyhow!("{e}"))
        .with_context(|| format!("failed to walk {}", cli.path.display()))?;

    let report = Report::build(root_name.clone(), as_of, &catalog, hits, stats);

    if cli.json {
        out.write_all(render_json(&report).as_bytes())?;
    } else if cli.quiet {
        // stdout stays a single pipeable line; the framing goes to stderr so it
        // is still shown without polluting the summary.
        writeln!(
            out,
            "sunsetscan: {}  {}",
            report.root_name,
            summary_line(&report, opts)
        )?;
        eprintln!(
            "Catalog verified as of {}; vendors move dates. Matching is textual, not semantic.",
            report.catalog_verified_on
        );
    } else {
        out.write_all(render_text(&report, opts).as_bytes())?;
    }
    out.flush()?;

    let policy = if cli.strict {
        FailPolicy::Any
    } else {
        FailPolicy::Within(cli.fail_within.unwrap_or(CRITICAL_DAYS))
    };
    Ok(report.exit_code(policy))
}
