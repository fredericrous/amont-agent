//! `amont-agent` — a guard that inspects a shell command before Claude Code
//! runs it.
//!
//! Amont gates `git commit` and `git push`. Neither can see the command string
//! itself, and some defects live only there: `git push … | tail -5` reports the
//! pipe's exit status, so a rejected push reads as success and no git hook is
//! ever in a position to notice.
//!
//! This binary is not on the commit path and takes dependencies. It is a
//! sibling of `amont-fleet` in that respect: opt-in, installed separately, and
//! free to buy features with crates.
//!
//! ## argv
//!
//! The parse follows `crates/amont/src/main.rs` exactly, including its
//! hardest-won rule: **only position 0 is ever tested against the subcommand
//! table.** Everything after it is data. In `amont` that rule exists because a
//! git remote named `install` once ran the installer mid-push; the same class
//! of accident is available to anything that scans argv for a verb.

use amont_agent_guard::analysis;
use amont_agent_guard::assertions;
use amont_agent_guard::backtest;
use amont_agent_guard::civil;
use amont_agent_guard::compliance;
use amont_agent_guard::corpus;
use amont_agent_guard::decision;
use amont_agent_guard::doctor;
use amont_agent_guard::graduate;
use amont_agent_guard::guidance;
use amont_agent_guard::harness_review;
use amont_agent_process::attestations;
use amont_agent_process::content_id;
mod hook;
use amont_agent_guard::journal;
use amont_agent_guard::json;
use amont_agent_guard::lsp;
use amont_agent_guard::mine;
use amont_agent_guard::payload;
use amont_agent_guard::push_target;
#[cfg(unix)]
use amont_agent_guard::quiet;
use amont_agent_process::impl_review_outcomes;
use amont_agent_process::implementation_review;
use amont_agent_process::implementation_review_rule;
use amont_agent_process::plan_apply;
use amont_agent_process::plan_phases;
use amont_agent_process::plan_review;
use amont_agent_process::plan_text;
use amont_agent_process::plans;
use amont_agent_process::preview;
use amont_agent_process::push_preview;
use amont_agent_process::review_brief;
use amont_agent_process::review_outcomes;
mod rule_table;
use amont_agent_guard::rules;
use amont_agent_guard::session_state;
use amont_agent_guard::settings;
use amont_agent_guard::shape;
use amont_agent_guard::shell;
use amont_agent_guard::shim;
use amont_agent_guard::stale;
use amont_agent_guard::stance;
use amont_agent_guard::transcript;
use amont_agent_guard::ui;
use amont_agent_process::shipped;

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
usage: amont-agent <command>

  hook                  read a Claude Code payload on stdin, decide
  install [--write]     add the hook to settings.json (prints it by default)
  uninstall [--write]   remove exactly what install added
  status                every rule, its stance, and what it has seen
  doctor [--json]       is the guard installed, runnable, and actually firing?
                        --json prints the findings and, apart from health, the
                        hook integration: present, absent, partial or unknown
  corpus check          replay every reviewed judgement through the rules
  graduate <rule> --to advise|deny
  demote <rule>         back to observing, no questions asked
  mine [flags]          shapes the model got wrong that no rule names yet
  backtest [flags]      replay your transcripts through the rules
  harness-review [--json] [--check] [--model <id>] [--out <dir>]
                        one row per rule and assertion: firings, compliance
                        per model, shadow-arm share, replay count; writes
                        <out dir>/<date>.json, --out defaulting to
                        <journal dir>/harness-review. --model is the model
                        the review is for (default: the model with the most
                        Bash calls in your transcripts, named in the output).
                        --check prints one line and exits 1 when that default
                        model is not the one the last review was made for
  explain <rule>        every match for one rule, for review
  check '<command>'     run the rules over one command, no stdin
                        [--dialect bash|zsh|unknown, default unknown]
                        [--cwd <dir>, default this directory] [--explain]
                        decide as the hook would in <dir>; --explain also
                        lists rules that matched but were not confirmed
  analyze '<command>'   the shell analysis of one command, as JSON — the
                        interface tools/shell-oracle compares against real
                        shells [--dialect, as for check]
  rules [--json]        every rule, its stance and its evidence; --json is
                        one array, the interface the homebrew tap reads
  preview register --url <url> --guide <file.md> [--repo <dir>] [--open]
                        validate a localhost preview of HEAD for approval
                        and render its guide to a page
  plan-sha [--short] [--block [--lang <l>]] [--legacy] [--] <file|->
                        the sha256 of a plan's canonical body, as reviewed
  plan-panel <plan.md>  the review agents a plan still needs, as the
                        plan-review-panel hook will judge it
  plan-review record <plan.md> [--transcript <path>]
                        keep what each reviewer of a plan did, as outcome rows
  plan-review stats [--json] [--model <id>]
                        per (role, model): reviews, findings, accepted ones;
                        marks a seat that is a retire-candidate
  implementation-review record <plan.md> --verdict <v>
                        keep the implementation review's findings (JSON on
                        stdin) as one outcome row
  plan-apply [--dry-run|--check|--record-fallback] <plan.md>
                        apply the reviewers' Patches to a plan with no model
                        (accepted findings on stdin; exit 3 = some unmatched)
  attestations [--apply]
                        what ~/.claude/amont-agent/attestations keeps and
                        why; --apply moves the rest to a 7-day trash
  tree-sha [--block] [-C <dir>] [--] [<rev>]
                        the content id an implementation review and a
                        preview approval bind to: HEAD's tree without
                        docs/plans/ (not attest's canonical tree)
  rules-sha [-C <dir>]  the sha256 of the code rules `aval` reports, as an
                        implementation-review brief binds them
  review-brief [-C <dir>] [--plan <path>] [--base <ref>]
                        the implementation-review brief: tree and rules
                        lines, plan, diff, full code rules, decisions
  shipped <plan.md> [--json]
                        what shipped under a plan, derived from git: per
                        repository its merge commits, PRs, releases, and
                        signed provenance
  quiet [--max-bytes N[k]] -- <cmd…>
                        run a command with its output in a log file: one line
                        on a pass, the failure on a fail, the real exit code.
                        N is 1 to 48k bytes of excerpt (default 16k); exits
                        1 wrapper failure, 2 usage, 126 cannot execute,
                        127 not found, the command's own code, or
                        128+signal

install/uninstall flags:
  --write               actually edit the file
  --reformat            accept a normalised file when we cannot match its style
  --project             .claude/settings.json instead of ~/.claude
  --local               .claude/settings.local.json

backtest/explain/mine flags:
  --transcripts <dir>   where the .jsonl transcripts live
  --since <YYYY-MM-DD>  ignore entries before this day
  --rule <id>           restrict to one rule (repeatable)
  --sample <n>          sample matches to print per rule
  --format cases        emit matches as reviewable case lines (explain, mine)
  --json                machine-readable output

mine flags:
  --window <n>          tool calls a correction may arrive within (default 3)
  --min-support <n>     calls a shape needs before it is proposed (default 5)
  --min-rate <0..1>     share of them that must have gone wrong (default 0.3)

explain flags:
  --rank novelty        label the matches least like the ones already reviewed

backtest flags:
  --compliance          per model, per week: what the model did NEXT after a
                        rule fired — did the advice change anything?
";

enum Sub {
    Hook,
    Install,
    Uninstall,
    Status,
    Doctor,
    Corpus,
    Graduate,
    Demote,
    Backtest,
    HarnessReview,
    Explain,
    Mine,
    Check,
    Analyze,
    Rules,
    Preview,
    PlanSha,
    PlanPanel,
    PlanReview,
    ImplementationReview,
    PlanApply,
    TreeSha,
    RulesSha,
    ReviewBrief,
    Attestations,
    Quiet,
    Shipped,
}

const SUBCOMMANDS: [(&str, Sub); 27] = [
    ("hook", Sub::Hook),
    ("install", Sub::Install),
    ("uninstall", Sub::Uninstall),
    ("status", Sub::Status),
    ("doctor", Sub::Doctor),
    ("corpus", Sub::Corpus),
    ("graduate", Sub::Graduate),
    ("demote", Sub::Demote),
    ("backtest", Sub::Backtest),
    ("harness-review", Sub::HarnessReview),
    ("explain", Sub::Explain),
    ("mine", Sub::Mine),
    ("check", Sub::Check),
    ("analyze", Sub::Analyze),
    ("rules", Sub::Rules),
    ("preview", Sub::Preview),
    ("plan-sha", Sub::PlanSha),
    ("plan-panel", Sub::PlanPanel),
    ("plan-review", Sub::PlanReview),
    ("implementation-review", Sub::ImplementationReview),
    ("plan-apply", Sub::PlanApply),
    ("tree-sha", Sub::TreeSha),
    ("rules-sha", Sub::RulesSha),
    ("review-brief", Sub::ReviewBrief),
    ("attestations", Sub::Attestations),
    ("quiet", Sub::Quiet),
    ("shipped", Sub::Shipped),
];

enum Invocation {
    Sub { name: Sub, args: Vec<OsString> },
    Help,
    Version,
    Usage(String),
}

fn parse(argv: Vec<OsString>) -> Invocation {
    let Some(first) = argv.first() else {
        return Invocation::Usage("no command given".into());
    };
    let head = first.to_string_lossy().into_owned();
    if head == "--help" || head == "-h" || head == "help" {
        return Invocation::Help;
    }
    if head == "--version" || head == "-V" {
        return Invocation::Version;
    }
    for (name, sub) in SUBCOMMANDS {
        if head == name {
            return Invocation::Sub {
                name: sub,
                args: argv[1..].to_vec(),
            };
        }
    }
    Invocation::Usage(format!("unknown command `{head}`"))
}

/// Die quietly when the reader hangs up, like every other Unix filter.
///
/// Rust ignores SIGPIPE at startup, so `amont-agent rules | head` panicked
/// with a backtrace the moment `head` closed the pipe — `println!` hit EPIPE
/// and EPIPE is a panic. Restoring SIGPIPE's default disposition makes the
/// exit what a shell user expects (killed by the signal, status 141).
///
/// It matters more here than in an ordinary filter. This binary's entire
/// claim is that it is composed and predictable in front of somebody's
/// shell; a stack trace out of `rules | head` reads as a crash in exactly
/// the tool you were deciding whether to trust with your commands. It is
/// also the one thing this crate did NOT inherit when it left the amont
/// workspace: amont's `main` has carried this since `list | head` panicked,
/// and the split copied the modules the crate imported rather than the ones
/// it should have.
///
/// `extern "C"` rather than a crate: std already links libc, and a signal
/// disposition is not worth a dependency. Windows has no SIGPIPE; there the
/// pipe-closed write fails an `Err` path instead of raising a signal, and
/// this is a no-op.
#[cfg(unix)]
fn die_on_sigpipe() {
    extern "C" {
        fn signal(signum: i32, handler: usize) -> usize;
    }
    const SIGPIPE: i32 = 13;
    const SIG_DFL: usize = 0;
    unsafe {
        signal(SIGPIPE, SIG_DFL);
    }
}

#[cfg(not(unix))]
fn die_on_sigpipe() {}

fn main() -> ExitCode {
    die_on_sigpipe();
    // Claude Code does not spawn us from git, but a session started inside a
    // git hook still carries GIT_DIR / GIT_WORK_TREE, and those beat
    // `current_dir` for every git command we run. `crate::git` builds
    // its own Commands and has no seam to pass an environment through, so the
    // scrub has to happen here. This is the same failure the test harness's
    // `strip_git_env` exists to prevent; it committed to the wrong repository
    // once.
    //
    // NAMED, not a `GIT_` prefix sweep. The sweep took `GIT_CONFIG_GLOBAL`
    // and `GIT_CONFIG_SYSTEM` with it, and those two say where config LIVES
    // rather than which repository this is — so a person who keeps their git
    // config somewhere other than `~/.gitconfig` had every stance they set
    // silently ignored, and this crate's own tests could not point the
    // reader at a fixture. amont's `rehearsal::strip_repo_env` draws the
    // line in the same place and for the same reason.
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_PREFIX",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_NAMESPACE",
        "GIT_REFLOG_ACTION",
        "GIT_EXEC_PATH",
    ] {
        std::env::remove_var(key);
    }

    match parse(std::env::args_os().skip(1).collect()) {
        Invocation::Help => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Invocation::Version => {
            println!("amont-agent {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Invocation::Usage(why) => {
            eprintln!("amont-agent: {why}\n");
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
        Invocation::Sub { name, args } => match name {
            // `hook` takes no flags: everything it needs arrives on stdin.
            // Claude Code may pass `--event <name>` for readability, and it is
            // accepted and ignored — the payload names its own event, and
            // trusting argv over the payload would let the two disagree.
            Sub::Hook => {
                let _ = args;
                hook::run()
            }
            Sub::Install => run_install(&args, true),
            Sub::Uninstall => run_install(&args, false),
            Sub::Status => run_status(),
            Sub::Doctor => {
                // Non-zero when the guard is not doing anything, so this is
                // usable from a cron job or a SessionStart check without
                // anybody having to read the text.
                if args.iter().any(|a| a == "--help" || a == "-h") {
                    println!("usage: amont-agent doctor [--json]");
                    println!();
                    println!("Check that the guard is installed and alive; exit 0 when it is.");
                    println!("  --json  print one JSON document: healthy, findings, integration");
                    return ExitCode::SUCCESS;
                }
                if let Some(bad) = args.iter().find(|a| *a != "--json") {
                    eprintln!(
                        "amont-agent: doctor takes only --json, not {}",
                        bad.to_string_lossy()
                    );
                    return ExitCode::from(2);
                }
                let scanned = doctor::scan();
                let mut findings =
                    doctor::run(rule_table::RULES, attestations::doctor_finding(), &scanned);
                findings.push(plan_root_finding());
                let healthy = if args.iter().any(|a| a == "--json") {
                    println!(
                        "{}",
                        doctor::json(&findings, &doctor::integration(&scanned))
                    );
                    doctor::healthy(&findings)
                } else {
                    doctor::report(&findings)
                };
                if healthy {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::FAILURE
                }
            }
            Sub::Corpus => run_corpus(&args),
            Sub::Graduate => run_graduate(&args, true),
            Sub::Demote => run_graduate(&args, false),
            Sub::Backtest => run_backtest(&args, false),
            Sub::HarnessReview => run_harness_review(&args),
            Sub::Explain => run_backtest(&args, true),
            Sub::Mine => run_mine(&args),
            Sub::Check => run_check(&args),
            Sub::Analyze => run_analyze(&args),
            Sub::Rules => run_rules(&args),
            Sub::Preview => run_preview(&args),
            Sub::PlanSha => run_plan_sha(&args),
            Sub::Shipped => shipped::run(&args),
            Sub::PlanPanel => run_plan_panel(&args),
            Sub::PlanReview => {
                let (said, code) = review_outcomes::run(&args);
                print!("{said}");
                code
            }
            Sub::ImplementationReview => {
                let (said, code) = impl_review_outcomes::run(&args);
                print!("{said}");
                code
            }
            Sub::PlanApply => {
                let (said, code) = plan_apply::run(&args);
                print!("{said}");
                code
            }
            Sub::TreeSha => run_tree_sha(&args),
            Sub::RulesSha => run_rules_sha(&args),
            Sub::ReviewBrief => run_review_brief(&args),
            Sub::Attestations => attestations::run_cli(&args),
            #[cfg(unix)]
            Sub::Quiet => quiet::run(&args),
            #[cfg(not(unix))]
            Sub::Quiet => {
                eprintln!("amont-agent: quiet is not supported on this platform");
                ExitCode::from(2)
            }
        },
    }
}

#[derive(Default)]
struct Flags {
    write: bool,
    reformat: bool,
    project: bool,
    local: bool,
    transcripts: Vec<PathBuf>,
    since: Option<civil::Day>,
    only: Vec<String>,
    sample: Option<usize>,
    json: bool,
    format: Option<String>,
    to: Option<String>,
    force: bool,
    window: Option<usize>,
    min_support: Option<u32>,
    min_rate: Option<f64>,
    rank: Option<String>,
    compliance: bool,
    check: bool,
    out: Option<PathBuf>,
    model: Option<String>,
    dialect: Option<rules::Dialect>,
    cwd: Option<PathBuf>,
    explain: bool,
    rest: Vec<String>,
}

fn flags(args: &[OsString]) -> Result<Flags, String> {
    let mut f = Flags::default();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].to_string_lossy().into_owned();
        let value = |i: &mut usize, what: &str| -> Result<String, String> {
            *i += 1;
            args.get(*i)
                .map(|v| v.to_string_lossy().into_owned())
                .ok_or_else(|| format!("{what} needs a value"))
        };
        match a.as_str() {
            "--out" => f.out = Some(PathBuf::from(value(&mut i, &a)?)),
            "--model" => f.model = Some(value(&mut i, &a)?),
            "--transcripts" => f.transcripts.push(PathBuf::from(value(&mut i, &a)?)),
            "--since" => {
                let v = value(&mut i, &a)?;
                f.since =
                    Some(civil::day_of(&v).ok_or_else(|| format!("--since: `{v}` is not a date"))?);
            }
            "--rule" => f.only.push(value(&mut i, &a)?),
            "--sample" => {
                let v = value(&mut i, &a)?;
                f.sample = Some(
                    v.parse()
                        .map_err(|_| format!("--sample: `{v}` is not a number"))?,
                );
            }
            "--window" => {
                let v = value(&mut i, &a)?;
                f.window = Some(
                    v.parse()
                        .map_err(|_| format!("--window: `{v}` is not a number"))?,
                );
            }
            "--min-support" => {
                let v = value(&mut i, &a)?;
                f.min_support = Some(
                    v.parse()
                        .map_err(|_| format!("--min-support: `{v}` is not a number"))?,
                );
            }
            "--min-rate" => {
                let v = value(&mut i, &a)?;
                let rate: f64 = v
                    .parse()
                    .map_err(|_| format!("--min-rate: `{v}` is not a number"))?;
                if !(0.0..=1.0).contains(&rate) {
                    return Err(format!("--min-rate: `{v}` is not a share between 0 and 1"));
                }
                f.min_rate = Some(rate);
            }
            "--rank" => f.rank = Some(value(&mut i, &a)?),
            "--dialect" => {
                let v = value(&mut i, &a)?;
                f.dialect = Some(
                    rules::Dialect::parse(&v)
                        .ok_or_else(|| format!("--dialect: `{v}` is not bash, zsh or unknown"))?,
                );
            }
            "--cwd" => f.cwd = Some(PathBuf::from(value(&mut i, &a)?)),
            "--explain" => f.explain = true,
            "--compliance" => f.compliance = true,
            "--check" => f.check = true,
            "--json" => f.json = true,
            "--format" => f.format = Some(value(&mut i, &a)?),
            "--to" => f.to = Some(value(&mut i, &a)?),
            "--force" => f.force = true,
            "--write" => f.write = true,
            "--reformat" => f.reformat = true,
            "--project" => f.project = true,
            "--local" => f.local = true,
            other if other.starts_with('-') => return Err(format!("unknown flag `{other}`")),
            other => f.rest.push(other.to_string()),
        }
        i += 1;
    }
    Ok(f)
}

/// Which rules and assertions a backtest replays.
///
/// Both kinds answer to an id, and somebody asking "how often would this have
/// fired" does not care which list it came from.
fn selected(only: &[String]) -> Result<Vec<backtest::Subject>, String> {
    let everything = || {
        rule_table::RULES
            .iter()
            .map(backtest::Subject::Rule)
            .chain(
                assertions::ASSERTIONS
                    .iter()
                    .map(backtest::Subject::Assertion),
            )
            .collect::<Vec<_>>()
    };
    if only.is_empty() {
        return Ok(everything());
    }
    only.iter()
        .map(|id| {
            rules::by_id(rule_table::RULES, id)
                .map(backtest::Subject::Rule)
                .or_else(|| assertions::by_id(id).map(backtest::Subject::Assertion))
                .ok_or_else(|| {
                    let known: Vec<&str> = everything().iter().map(|s| s.id()).collect();
                    format!("no rule `{id}` — known rules: {}", known.join(", "))
                })
        })
        .collect()
}

fn run_backtest(args: &[OsString], explain: bool) -> ExitCode {
    let mut f = match flags(args) {
        Ok(f) => f,
        Err(why) => {
            eprintln!("amont-agent: {why}");
            return ExitCode::from(2);
        }
    };
    if explain {
        match f.rest.len() {
            1 => f.only.push(f.rest[0].clone()),
            _ => {
                eprintln!("amont-agent: explain needs exactly one rule");
                return ExitCode::from(2);
            }
        }
    }
    let chosen = match selected(&f.only) {
        Ok(c) => c,
        Err(why) => {
            eprintln!("amont-agent: {why}");
            return ExitCode::from(2);
        }
    };
    let roots = match transcript::roots(&f.transcripts) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("amont-agent: {}", e.explain());
            return ExitCode::from(2);
        }
    };
    let scan = transcript::Scan {
        roots: &roots,
        tool: "Bash",
        since: f.since,
    };
    let as_cases = explain && f.format.as_deref() == Some("cases");
    if let Some(other) = f.format.as_deref() {
        if other != "cases" {
            eprintln!("amont-agent: --format takes `cases`, not `{other}`");
            return ExitCode::from(2);
        }
    }
    let novelty = match f.rank.as_deref() {
        None => false,
        Some("novelty") if explain => true,
        Some("novelty") => {
            eprintln!("amont-agent: --rank novelty is for `explain`, which prints the matches");
            return ExitCode::from(2);
        }
        Some(other) => {
            eprintln!("amont-agent: --rank takes `novelty`, not `{other}`");
            return ExitCode::from(2);
        }
    };
    if f.compliance {
        return run_compliance(&f, &roots, &scan, &chosen);
    }
    // A review dump wants everything, not a sample: the point is to look at
    // each match once and decide. Novelty ranking is the exception — it
    // answers "which twenty are worth an hour", and ordering every match by
    // novelty is quadratic in a number that runs to thousands.
    let samples = if novelty {
        usize::MAX
    } else {
        f.sample.unwrap_or(if as_cases {
            usize::MAX
        } else if explain {
            40
        } else {
            2
        })
    };
    match backtest::run(&scan, &chosen, samples) {
        Ok(mut report) => {
            if novelty {
                rank_by_novelty(&mut report, f.sample.unwrap_or(20));
            }
            if as_cases {
                // Every line starts unreviewed. The file this is appended to is
                // the same format the reviewer edits in place — one format, so
                // the review is a single pass with no export step to forget.
                println!("{}", corpus::HEADER);
                let mut seen = std::collections::BTreeSet::new();
                for group in &report.samples {
                    for sample in group {
                        if seen.insert(sample.command.clone()) {
                            print!(
                                "{}",
                                corpus::line_for(corpus::Verdict::Unreviewed, &sample.command)
                            );
                        }
                    }
                }
            } else if f.json {
                println!("{}", report.to_json(&roots));
            } else {
                print!("{}", report.render(&roots));
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("amont-agent: {}", e.explain());
            ExitCode::from(2)
        }
    }
}

/// Keep the `n` matches least like the cases already reviewed, and least like
/// each other.
///
/// The default sample is the first `n` matches the walk met, which is the
/// oldest project, the oldest session, and — because a habit repeats — very
/// often twenty spellings of one command. Labelling those costs the same
/// hour and moves the precision estimate by almost nothing. This picks the
/// hour's worth that moves it most.
fn rank_by_novelty(report: &mut backtest::Report, n: usize) {
    for (i, subject) in report.rules.iter().enumerate() {
        // Already-labelled cases are seeds: they are the neighbourhoods the
        // corpus already covers, and a second review pass should not hand
        // back the first pass's cases.
        let seeds: Vec<shape::Shape> = corpus::read(subject.id())
            .iter()
            .map(|c| shape::Shape::of_command(&c.command))
            .collect();
        let candidates: Vec<shape::Shape> = report.samples[i]
            .iter()
            .map(|s| shape::Shape::of_command(&s.command))
            .collect();
        let picked = shape::pick_novel(&candidates, &seeds, n);
        // In the greedy order, which is "most novel first": a reviewer who
        // stops halfway has still labelled the half that was worth most.
        let mut held: Vec<Option<backtest::Sample>> =
            report.samples[i].drain(..).map(Some).collect();
        report.samples[i] = picked.iter().filter_map(|&j| held[j].take()).collect();
    }
}

/// `harness-review`: the ADR-0042 review table, or with `--check` the one line
/// that says a model has not been reviewed yet.
fn run_harness_review(args: &[OsString]) -> ExitCode {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("usage: amont-agent harness-review [--json] [--check] [--model <id>] [--transcripts <dir>] [--since <date>] [--out <dir>]");
        return ExitCode::SUCCESS;
    }
    let f = match flags(args) {
        Ok(f) => f,
        Err(why) => {
            eprintln!("amont-agent: {why}");
            return ExitCode::from(2);
        }
    };
    if let Some(operand) = f.rest.first() {
        eprintln!("amont-agent: harness-review takes no operand, got `{operand}`");
        return ExitCode::from(2);
    }
    let Some(journal_dir) = journal::dir() else {
        eprintln!("amont-agent: no home directory, so no journal directory");
        return ExitCode::from(2);
    };
    let roots = match transcript::roots(&f.transcripts) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("amont-agent: {}", e.explain());
            return ExitCode::from(2);
        }
    };
    let scan = transcript::Scan {
        roots: &roots,
        tool: "Bash",
        since: f.since,
    };
    let out = f
        .out
        .clone()
        .unwrap_or_else(|| harness_review::reviews_dir(&journal_dir));
    if f.check {
        return match harness_review::check(&out, &scan) {
            Ok(None) => ExitCode::SUCCESS,
            Ok(Some(line)) => {
                println!("{line}");
                ExitCode::FAILURE
            }
            Err(e) => {
                eprintln!("amont-agent: {}", e.explain());
                ExitCode::from(2)
            }
        };
    }
    let today = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => (d.as_secs() / 86_400) as civil::Day,
        Err(e) => {
            eprintln!("amont-agent: cannot read the clock: {e}");
            return ExitCode::from(2);
        }
    };
    let table: Vec<&'static rules::Rule> = rule_table::RULES.iter().collect();
    let review = match harness_review::review(
        &scan,
        &table,
        stance_of_part,
        &journal::paths(),
        today,
        f.model.as_deref(),
    ) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("amont-agent: {}", e.explain());
            return ExitCode::from(2);
        }
    };
    if let Err(e) = harness_review::write(&out, &review) {
        eprintln!("amont-agent: could not write the review file: {e}");
        return ExitCode::from(2);
    }
    if f.json {
        println!("{}", review.to_json());
    } else {
        print!("{}", review.render());
    }
    ExitCode::SUCCESS
}

fn stance_of_part(part: &backtest::Subject) -> rules::Stance {
    match part {
        backtest::Subject::Rule(r) => stance::resolve(r),
        backtest::Subject::Assertion(a) => stance::resolve_assertion(a),
    }
}

fn run_compliance(
    f: &Flags,
    roots: &transcript::Roots,
    scan: &transcript::Scan,
    chosen: &[backtest::Subject],
) -> ExitCode {
    // Assertions check what a command that already ran actually did, so
    // "what did the model do next" is a different question with a different
    // answer. Named rather than silently dropped.
    let rules: Vec<&'static rules::Rule> = chosen
        .iter()
        .filter_map(|s| match s {
            backtest::Subject::Rule(r) => Some(*r),
            backtest::Subject::Assertion(_) => None,
        })
        .collect();
    if rules.is_empty() {
        eprintln!("amont-agent: --compliance is about rules, and none were selected");
        return ExitCode::from(2);
    }
    let options = compliance::Options {
        window: f.window.unwrap_or(compliance::Options::default().window),
    };
    match compliance::run(scan, &rules, options) {
        Ok(report) => {
            if f.json {
                println!("{}", report.to_json(roots));
            } else {
                print!("{}", report.render(roots));
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("amont-agent: {}", e.explain());
            ExitCode::from(2)
        }
    }
}

fn run_mine(args: &[OsString]) -> ExitCode {
    let f = match flags(args) {
        Ok(f) => f,
        Err(why) => {
            eprintln!("amont-agent: {why}");
            return ExitCode::from(2);
        }
    };
    if !f.only.is_empty() {
        eprintln!("amont-agent: mine looks for what no rule names yet, so --rule means nothing");
        return ExitCode::from(2);
    }
    let as_cases = f.format.as_deref() == Some("cases");
    if let Some(other) = f.format.as_deref() {
        if other != "cases" {
            eprintln!("amont-agent: --format takes `cases`, not `{other}`");
            return ExitCode::from(2);
        }
    }
    let roots = match transcript::roots(&f.transcripts) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("amont-agent: {}", e.explain());
            return ExitCode::from(2);
        }
    };
    let scan = transcript::Scan {
        roots: &roots,
        tool: "Bash",
        since: f.since,
    };
    let default = mine::Options::default();
    let options = mine::Options {
        window: f.window.unwrap_or(default.window),
        min_support: f.min_support.unwrap_or(default.min_support),
        min_rate: f.min_rate.unwrap_or(default.min_rate),
        samples: f.sample.unwrap_or(default.samples),
    };
    match mine::run(&scan, options, rule_table::RULES) {
        Ok(report) => {
            if as_cases {
                // The same file `explain --format cases` writes, and the same
                // one `corpus check` reads — so a shape mined today is a
                // reviewable case today, before anybody has written the rule
                // it might become.
                println!("{}", corpus::HEADER);
                let mut seen = std::collections::BTreeSet::new();
                for command in report.sample_commands() {
                    if seen.insert(command.to_string()) {
                        print!("{}", corpus::line_for(corpus::Verdict::Unreviewed, command));
                    }
                }
            } else if f.json {
                println!("{}", report.to_json(&roots));
            } else {
                print!("{}", report.render(&roots));
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("amont-agent: {}", e.explain());
            ExitCode::from(2)
        }
    }
}

/// The analysis of one command as JSON: what `tools/shell-oracle` checks
/// against bash and zsh actually running it. Counts are per contribution,
/// never aggregated — aggregation is policy.
fn run_analyze(args: &[OsString]) -> ExitCode {
    let f = match flags(args) {
        Ok(f) => f,
        Err(why) => {
            eprintln!("amont-agent: {why}");
            return ExitCode::from(2);
        }
    };
    if f.rest.len() != 1 {
        eprintln!("amont-agent: analyze needs exactly one command, quoted");
        return ExitCode::from(2);
    }
    let src = &f.rest[0];
    let dialect = f.dialect.unwrap_or(rules::Dialect::Unknown);
    let a = analysis::analyze(src, dialect);
    let upper = |u: analysis::domain::Upper| match u {
        analysis::domain::Upper::Finite(n) => serde_json::json!(n),
        analysis::domain::Upper::Saturated => serde_json::json!("saturated"),
        analysis::domain::Upper::Uncapped(_) => serde_json::json!("uncapped"),
        analysis::domain::Upper::Unknown => serde_json::json!("unknown"),
    };
    let contributions: Vec<serde_json::Value> = a
        .effects
        .contributions
        .iter()
        .map(|c| {
            serde_json::json!({
                "program": c.program,
                "span": [c.span.start, c.span.end],
                "hosts": c.targets.hosts,
                "unresolved": !c.targets.unresolved.is_empty(),
                "local": c.targets.local,
                "lower": c.transfers.lower,
                "upper": upper(c.transfers.upper),
                "paced": c.paced.map(|p| p.secs),
            })
        })
        .collect();
    let out = serde_json::json!({
        "dialect": dialect.as_str(),
        "parsed": analysis::frontend::parse(src).is_ok(),
        "incomplete": a.incomplete.as_ref().map(|i| i.why),
        "unknown": a.effects.unknown.len(),
        "contributions": contributions,
    });
    println!("{out}");
    ExitCode::SUCCESS
}

const PLAN_SHA_USAGE: &str = "\
usage: amont-agent plan-sha [--short] [--block [--lang <language>]] [--legacy] [--] <file|->

The sha256 of what a plan says, which its review binds to (ADR-0022,
work.plan-review-panel). The plan is read without its YAML front matter,
its `## Review panel` section, its `## Full reviews` section and its
machine comment (`<!-- panel: ... -->` on the last line), so writing review
results into it never changes the sha. The rest is read as CommonMark:

  counts         every word, number and operator; code blocks and code
                 spans exactly as written; link destinations; the kind of
                 each block (list item, quote, heading, table cell, an
                 ordered list's start)
  does not count blank lines, line wrapping, indentation outside code,
                 table padding, list-marker style (`*`/`-`), emphasis
                 style (`*x*`/`_x_`), escapes (`\\_`), CRLF

So a formatter such as prettier keeps the sha, unless it rewrites code.

  --short            the first 12 hex characters, for the machine comment
  --block            the review block a plan-review-* agent's prompt carries:
                     <<<PLAN path=<realpath> sha=<64 hex>>>>
  --lang <language>  with --block, for plan-review-language: adds lang=<l>
  --legacy           the byte sha from before 2.25, to check a plan whose
                     machine comment was written then; temporary, removed
                     after the transition (not with --block)
  -                  read the plan from stdin (not with --block, which
                     needs the file's path)

Prints one line on stdout. Exit 0, 1 when the file cannot be read or its
sha cannot be computed (the reason on stderr), 2 on a usage error.
";

fn run_plan_sha(args: &[OsString]) -> ExitCode {
    let args: Vec<String> = args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let (mut short, mut block, mut lang, mut file) = (false, false, None, None);
    let mut legacy = false;
    let mut i = 0;
    let mut operands_only = false;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            _ if operands_only || a == "-" || !a.starts_with('-') => {
                if file.is_some() {
                    eprintln!(
                        "amont-agent: plan-sha takes one file; try `amont-agent plan-sha --help`"
                    );
                    return ExitCode::from(2);
                }
                file = Some(a.to_string());
            }
            "--" => operands_only = true,
            "--help" | "-h" => {
                print!("{PLAN_SHA_USAGE}");
                return ExitCode::SUCCESS;
            }
            "--short" => short = true,
            "--block" => block = true,
            "--legacy" => legacy = true,
            "--lang" => {
                i += 1;
                match args.get(i) {
                    Some(v) => lang = Some(v.clone()),
                    None => {
                        eprintln!("amont-agent: --lang needs a value");
                        return ExitCode::from(2);
                    }
                }
            }
            other => {
                eprintln!("amont-agent: unknown flag `{other}`; try `amont-agent plan-sha --help`");
                return ExitCode::from(2);
            }
        }
        i += 1;
    }
    let Some(file) = file else {
        eprint!("amont-agent: plan-sha needs a file, or - for stdin\n\n{PLAN_SHA_USAGE}");
        return ExitCode::from(2);
    };
    let conflict = if short && block {
        Some("--short and --block exclude each other")
    } else if lang.is_some() && !block {
        Some("--lang only goes with --block")
    } else if block && file == "-" {
        Some("--block needs the plan's file, not stdin")
    } else if block && legacy {
        Some("--legacy only prints a sha; drop --block")
    } else {
        None
    };
    if let Some(why) = conflict {
        eprintln!("amont-agent: {why}; try `amont-agent plan-sha --help`");
        return ExitCode::from(2);
    }
    let text = if file == "-" {
        use std::io::Read;
        let mut t = String::new();
        if let Err(e) = std::io::stdin().read_to_string(&mut t) {
            eprintln!("amont-agent: cannot read stdin: {e}");
            return ExitCode::from(1);
        }
        t
    } else {
        match std::fs::read_to_string(&file) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("amont-agent: cannot read {file}: {e}");
                return ExitCode::from(1);
            }
        }
    };
    let sha = if legacy {
        eprintln!("amont-agent: --legacy is removed after the transition");
        plan_text::legacy_sha(&text)
    } else {
        match plan_text::shas(&text) {
            Ok(s) => s.sha,
            Err(why) => {
                eprintln!("amont-agent: {why}");
                return ExitCode::from(1);
            }
        }
    };
    if block {
        let path = plan_review::real(std::path::Path::new(&file));
        println!("{}", plan_review::block(&path, &sha, lang.as_deref()));
    } else if short {
        println!("{}", &sha[..12]);
    } else {
        println!("{sha}");
    }
    ExitCode::SUCCESS
}

const TREE_SHA_USAGE: &str = "\
usage: amont-agent tree-sha [--block] [-C <dir>] [--] [<rev>]

The content id an implementation review and a preview approval bind to
(not attest's canonical tree): the sha256 of `git ls-tree -r -z` of <rev>
(default HEAD) with every entry under docs/plans/ left out, 64 hex in any
object format (attest SPEC, \"Content id\"). Recording the review in the
plan, or adding a plan, therefore never changes it; a code change does
(ADR-0022, ADR-0034, work.implementation-review).

  --block    the review block the implementation-review agent's prompt
             carries: <<<TREE repo=<name> sha=<64 hex>>>>, where <name>
             is the basename of the directory holding the common .git
  -C <dir>   answer for the repository at <dir> (default: the current one)

Prints one line on stdout. Exit 0; 1 outside a repository or for a rev
that names no tree (the reason on stderr); 2 on a usage error.
";

fn run_tree_sha(args: &[OsString]) -> ExitCode {
    let args: Vec<String> = args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let (mut block, mut dir, mut rev) = (false, None, None);
    let mut i = 0;
    let mut operands_only = false;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            _ if operands_only || !a.starts_with('-') => {
                if rev.is_some() {
                    eprintln!(
                        "amont-agent: tree-sha takes one rev; try `amont-agent tree-sha --help`"
                    );
                    return ExitCode::from(2);
                }
                rev = Some(a.to_string());
            }
            "--" => operands_only = true,
            "--help" | "-h" => {
                print!("{TREE_SHA_USAGE}");
                return ExitCode::SUCCESS;
            }
            "--block" => block = true,
            "-C" => {
                i += 1;
                match args.get(i) {
                    Some(v) => dir = Some(std::path::PathBuf::from(v)),
                    None => {
                        eprintln!("amont-agent: -C needs a directory");
                        return ExitCode::from(2);
                    }
                }
            }
            other => {
                eprintln!("amont-agent: unknown flag `{other}`; try `amont-agent tree-sha --help`");
                return ExitCode::from(2);
            }
        }
        i += 1;
    }
    let dir = dir.unwrap_or_else(|| {
        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
    });
    let Some(repo) = push_target::toplevel(&dir) else {
        eprintln!(
            "amont-agent: {} is not inside a git repository",
            dir.display()
        );
        return ExitCode::from(1);
    };
    let rev = rev.as_deref().unwrap_or("HEAD");
    let sha = match content_id::content_id(&repo, rev) {
        Ok(s) => s,
        Err(why) => {
            eprintln!("amont-agent: {why}");
            return ExitCode::from(1);
        }
    };
    if block {
        let Some(name) = implementation_review::repo_name(&repo) else {
            eprintln!(
                "amont-agent: the repository at {} has no name (git rev-parse --git-common-dir failed)",
                repo.display()
            );
            return ExitCode::from(1);
        };
        println!("{}", implementation_review::block(&name, &sha));
    } else {
        println!("{sha}");
    }
    ExitCode::SUCCESS
}

const RULES_SHA_USAGE: &str = "\
usage: amont-agent rules-sha [-C <dir>]

The lowercase sha256 hex of the stdout of
`aval --json rules --level constraint --governs code --full`, run in <dir>
(default: the current directory). An implementation-review brief carries it
as <<<RULES sha=<64 hex>>>>; the push check compares it.

Prints one line on stdout. Exit 0; 3 when aval is missing or fails (the
command on stderr, nothing on stdout); 2 on a usage error.
";

const REVIEW_BRIEF_USAGE: &str = "\
usage: amont-agent review-brief [-C <dir>] [--plan <path>] [--base <ref>]

The markdown brief an implementation reviewer is launched with: the
<<<TREE ...>>> block, the <<<RULES sha=...>>> line and adr-lint's
<<<CANDIDATES ...>>> block (paste them verbatim into the reviewer prompt),
the plan's Verification section, the diff since the merge-base, the
candidates, the code rules in full, the constraint rules, the decisions that
bear on the changed files, and the plan's Non-goals. A repository with no
.adr.yaml gets no rules line and a note that no decided code rules apply.
adr-lint is `adr-lint` from PATH, or $AMONT_AGENT_ADR_LINT; when it cannot
answer, the block is <<<CANDIDATES unavailable>>> and the brief goes on.

  -C <dir>        the repository (default: the current directory)
  --plan <path>   the plan (default: the one file under docs/plans/ changed
                  since the merge-base; the brief says when there is none
                  or several)
  --base <ref>    the base ref (default: the remote's default branch)

Exit 0; 1 when the repository or its base cannot be read; 3 when aval is missing or fails (nothing on
stdout); 2 on a usage error.
";

/// The flags of `rules-sha` and `review-brief`.
struct BriefFlags {
    dir: std::path::PathBuf,
    plan: Option<std::path::PathBuf>,
    base: Option<String>,
}

/// One subcommand's name, usage and accepted flags, kept together.
struct BriefCommand {
    name: &'static str,
    usage: &'static str,
    flags: &'static [&'static str],
}

/// `rules-sha` takes `-C` alone.
const RULES_SHA_COMMAND: BriefCommand = BriefCommand {
    name: "rules-sha",
    usage: RULES_SHA_USAGE,
    flags: &["-C"],
};

const REVIEW_BRIEF_COMMAND: BriefCommand = BriefCommand {
    name: "review-brief",
    usage: REVIEW_BRIEF_USAGE,
    flags: &["-C", "--plan", "--base"],
};

fn brief_flags(command: &BriefCommand, args: &[OsString]) -> Result<BriefFlags, ExitCode> {
    let BriefCommand {
        name,
        usage,
        flags: accepted,
    } = command;
    let args: Vec<String> = args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{usage}");
        return Err(ExitCode::SUCCESS);
    }
    let mut flags = BriefFlags {
        dir: std::path::PathBuf::from("."),
        plan: None,
        base: None,
    };
    let mut i = 0;
    while i < args.len() {
        let word = args[i].as_str();
        i += 1;
        // Neither command takes an operand, so `--` only ends the flags.
        if word == "--" {
            if let Some(operand) = args.get(i) {
                eprintln!(
                    "amont-agent: {name}: unknown argument `{operand}`; try `amont-agent {name} --help`"
                );
                return Err(ExitCode::from(2));
            }
            break;
        }
        // POSIX shapes: `--name=value`, and a short flag's value attached as `-C<dir>`.
        let (flag, attached) = match word.split_once('=') {
            Some((long, value)) if long.starts_with("--") => (long, Some(value.to_string())),
            // `get` keeps a word whose second byte is inside a character a usage error.
            _ => match (word.get(..2), word.get(2..)) {
                (Some(short), Some(rest))
                    if !rest.is_empty() && !word.starts_with("--") && accepted.contains(&short) =>
                {
                    (short, Some(rest.to_string()))
                }
                _ => (word, None),
            },
        };
        if !accepted.contains(&flag) {
            eprintln!(
                "amont-agent: {name}: unknown argument `{word}`; try `amont-agent {name} --help`"
            );
            return Err(ExitCode::from(2));
        }
        let value = match attached {
            Some(value) => value,
            None => {
                let Some(value) = args.get(i) else {
                    eprintln!("amont-agent: {name}: {flag} needs a value");
                    return Err(ExitCode::from(2));
                };
                i += 1;
                value.clone()
            }
        };
        match flag {
            "-C" => flags.dir = std::path::PathBuf::from(value),
            "--plan" => flags.plan = Some(std::path::PathBuf::from(value)),
            "--base" => flags.base = Some(value),
            _ => unreachable!("`accepted` holds only -C, --plan and --base"),
        }
    }
    Ok(flags)
}

fn run_rules_sha(args: &[OsString]) -> ExitCode {
    let flags = match brief_flags(&RULES_SHA_COMMAND, args) {
        Ok(f) => f,
        Err(code) => return code,
    };
    match review_brief::rules_sha(&flags.dir) {
        Ok(sha) => {
            println!("{sha}");
            ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("amont-agent: {why}");
            ExitCode::from(3)
        }
    }
}

fn run_review_brief(args: &[OsString]) -> ExitCode {
    let flags = match brief_flags(&REVIEW_BRIEF_COMMAND, args) {
        Ok(f) => f,
        Err(code) => return code,
    };
    let request = review_brief::BriefRequest {
        dir: flags.dir,
        plan: flags.plan,
        base: flags.base,
    };
    match review_brief::build(&request) {
        Ok(brief) => {
            for notice in &brief.notices {
                eprintln!("amont-agent: review-brief: {notice}");
            }
            print!("{}", brief.text);
            ExitCode::SUCCESS
        }
        Err(review_brief::BriefError::Aval(why)) => {
            eprintln!("amont-agent: {why}");
            ExitCode::from(3)
        }
        Err(review_brief::BriefError::Repository(why)) => {
            eprintln!("amont-agent: {why}");
            ExitCode::from(1)
        }
    }
}

const PLAN_PANEL_USAGE: &str = "\
usage: amont-agent plan-panel <plan.md>

The review agents a plan still needs, computed as the plan-review-panel
hook will judge it at ExitPlanMode: the areas of every repository in the
plan's machine comment (`<!-- panel: repos=... adds=... -->`), and whether
this is the whole panel or a delta since the plan's last accepted body.

First line: repos=<names> areas=<areas> body=<12 hex> panel=full|delta|current
Then one line per agent to launch: `plan-review-<role>`, and `--lang <l>`
for a language review (the flag its `plan-sha --block` call takes).

Exit 0, 1 when the plan cannot be read or its comment is wrong (the reason
on stderr), 2 on a usage error.
";

fn run_plan_panel(args: &[OsString]) -> ExitCode {
    let args: Vec<String> = args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{PLAN_PANEL_USAGE}");
        return ExitCode::SUCCESS;
    }
    let [file] = args.as_slice() else {
        eprint!("amont-agent: plan-panel takes one plan file\n\n{PLAN_PANEL_USAGE}");
        return ExitCode::from(2);
    };
    match plan_review::panel(std::path::Path::new(file)) {
        Ok(p) => {
            let areas = p.classes.iter().cloned().collect::<Vec<_>>().join(",");
            println!(
                "repos={} areas={} body={} panel={}",
                p.repos,
                if areas.is_empty() { "-" } else { &areas },
                &p.sha[..12],
                p.mode
            );
            for role in &p.needed {
                match plan_review::agent_for(role) {
                    (agent, Some(lang)) => println!("{agent} --lang {lang}"),
                    (agent, None) => println!("{agent}"),
                }
            }
            ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("amont-agent: {why}");
            ExitCode::from(1)
        }
    }
}

const PREVIEW_USAGE: &str = "\
usage: amont-agent preview register --url <url> --guide <file.md> [--repo <dir>] [--open]

Validate a localhost preview of the repository's HEAD, for the person to
approve before a UI-changing push (ADR-0023), and render its guide to a
page (work.preview-is-guided). Run it in the foreground as the last
command of its line (`cd <wt> && npm run build && amont-agent preview
register …` is fine; nothing piped, redirected or after it): the Claude
Code hook binds its output to the session.

  --url <url>         where the clean worktree is served (http or https)
  --guide <file.md>   what the person needs to decide with. Must be OUTSIDE
                      the worktree (it and its page would dirty the commit
                      they describe) and carry these H2 sections, emoji
                      and case aside:
                        ## Where we are          project, branch, plan, why
                                                 they are asked
                        ## What you should see   in their words; \"nothing
                                                 new\" when that is the point
                        ## Try it                numbered steps (`1. ...`):
                                                 the URL, each click by its
                                                 label, what should appear
                        ## Reference             ![before](before.png)
                                                 ![after](after.png)
                        ## Already checked       what you verified, and what
                                                 to look at especially
                      A heading may carry a note: `## Try it (2 min)`.
                      Images are relative to the guide's directory; two
                      named *before* and *after* show side by side.

                      Mockup mode: when the branch (from its merge base
                      with the default branch) commits a picked mockup's
                      artboards (`*.dc.html` under docs/mockups/<screen>, or
                      `git config amont.agent.preview.mockups`), the guide
                      must also show, under ## Reference, the screen path,
                      `Viewport: <width>px · <theme>`, and the artboard
                      beside the built screen as files next to it
                      (artboard.png / after.png; also mockup / live, and
                      state pairs such as artboard-empty.png /
                      after-empty.png), plus a section
                        ## Differences from the mockup
                                                 `None`, or bullets
                                                 `- fixed: …` /
                                                 `- deliberate: <reason>`
                      Copy the images beside the guide as their own step,
                      before this command.
  --repo <dir>        the repository (default: the current directory)
  --open              open the rendered page with the platform opener; a
                      failure to open never fails the command
  --attestation <f>   DEPRECATED alias of --guide, for one release: read as
                      the guide, and refused unless it is one

Writes index.html beside the guide and prints one JSON object, on one
line, on stdout: id, repo, commit, url, guide, page, page_url, label,
aliases, question_prefix. Start the marked question with `question_prefix`
(`[preview <id>] <label>`), verbatim: the id is what approves. Exit 0 when
valid, 1 when refused (the reason on stderr), 2 on a usage error.

example:
  amont-agent preview register --url http://localhost:5173/settings \\
    --guide ~/.claude/amont-agent/attestations/abc1234/guide.md
";

fn run_preview(args: &[OsString]) -> ExitCode {
    let args: Vec<String> = args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    if args.is_empty() || args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{PREVIEW_USAGE}");
        return if args.is_empty() {
            ExitCode::from(2)
        } else {
            ExitCode::SUCCESS
        };
    }
    if args[0] != "register" {
        eprintln!(
            "amont-agent: unknown preview command `{}`; try `amont-agent preview --help`",
            args[0]
        );
        return ExitCode::from(2);
    }
    let (mut url, mut guide, mut attestation, mut repo) = (None, None, None, None);
    let mut open = false;
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--open" {
            open = true;
            i += 1;
            continue;
        }
        let (flag, inline) = match args[i].split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (args[i].clone(), None),
        };
        let value = match inline {
            Some(v) => v,
            None => {
                i += 1;
                match args.get(i) {
                    Some(v) => v.clone(),
                    None => {
                        eprintln!("amont-agent: {flag} needs a value");
                        return ExitCode::from(2);
                    }
                }
            }
        };
        match flag.as_str() {
            "--url" => url = Some(value),
            "--guide" => guide = Some(value),
            "--attestation" => attestation = Some(value),
            "--repo" => repo = Some(value),
            other => {
                eprintln!("amont-agent: unknown flag `{other}`; try `amont-agent preview --help`");
                return ExitCode::from(2);
            }
        }
        i += 1;
    }
    let guide = match (guide, attestation) {
        (Some(g), None) => g,
        (Some(g), Some(_)) => {
            eprintln!("amont-agent: --attestation is deprecated and ignored beside --guide");
            g
        }
        (None, Some(a)) => {
            eprintln!(
                "amont-agent: --attestation is deprecated; use --guide. It is read as the guide, and refused unless it is one (see `amont-agent preview --help`)"
            );
            a
        }
        (None, None) => {
            eprintln!(
                "amont-agent: preview register needs --guide <file.md>: the person is guided, not quizzed (see `amont-agent preview --help`)"
            );
            return ExitCode::from(2);
        }
    };
    let Some(url) = url else {
        eprintln!("amont-agent: preview register needs --url");
        return ExitCode::from(2);
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let dir = repo.map(PathBuf::from).unwrap_or_else(|| cwd.clone());
    let dir = if dir.is_absolute() {
        dir
    } else {
        cwd.join(dir)
    };
    match preview::register(&dir, &url, std::path::Path::new(&guide)) {
        Ok(r) => {
            println!("{}", r.to_json());
            if open {
                open_page(std::path::Path::new(&r.page));
            }
            ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("amont-agent: preview not registered: {why}");
            ExitCode::from(1)
        }
    }
}

/// Hand the page to the platform's opener and do not wait. Whatever
/// happens, the registration stands: the page is a convenience.
fn open_page(page: &std::path::Path) {
    use std::process::{Command, Stdio};
    let mut cmd = if cfg!(target_os = "macos") {
        Command::new("open")
    } else if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/c", "start", ""]);
        c
    } else {
        Command::new("xdg-open")
    };
    let spawned = cmd
        .arg(page)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if spawned.is_err() {
        eprintln!(
            "amont-agent: the page could not be opened; it is at {}",
            page.display()
        );
    }
}

fn run_check(args: &[OsString]) -> ExitCode {
    let f = match flags(args) {
        Ok(f) => f,
        Err(why) => {
            eprintln!("amont-agent: {why}");
            return ExitCode::from(2);
        }
    };
    if f.rest.len() != 1 {
        eprintln!("amont-agent: check needs exactly one command, quoted");
        return ExitCode::from(2);
    }
    let src = &f.rest[0];
    let parsed = shell::lex(src);
    // The dialect is what the command would run in. `check` has no hook
    // payload and no business guessing from its own `SHELL`, so it is told,
    // or it says `unknown` — the same answer the backtester has to give.
    let input = rules::Input::new(src, f.dialect.unwrap_or(rules::Dialect::Unknown), &parsed);
    let found = rules::evaluate(rule_table::RULES, &input);
    if found.is_empty() {
        match &parsed {
            shell::Parsed::Opaque(why) => println!("no opinion: {}", why.why()),
            _ => println!("no rule fires"),
        }
        return ExitCode::SUCCESS;
    }
    let cwd = match f.cwd.clone().map(Ok).unwrap_or_else(std::env::current_dir) {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("amont-agent: check: the working directory cannot be read: {e}");
            return ExitCode::from(2);
        }
    };
    let mut printed = 0;
    for (rule, finding) in found {
        // The stance in force on THIS machine, which is what the hook would
        // do — with the shipped one beside it when they differ. Printing
        // only the default read `[advise]` for a rule long graduated to deny,
        // and the reader took it for the verdict.
        let mut now = stance::resolve(rule);
        let mut reason = finding.reason.clone();
        let mut remedy = finding.remedy.clone();
        if rule.confirm.is_some() {
            match hook::confirmed_for_check(rule, &finding, src, &cwd, &parsed) {
                Ok(checked) => {
                    now = checked.stance;
                    if let Some((said, said_remedy)) = checked.said {
                        reason = said;
                        if let Some(said_remedy) = said_remedy {
                            remedy = said_remedy;
                        }
                    }
                }
                Err(why) => {
                    if f.explain {
                        println!("{} not confirmed: {why}", rule.id);
                    }
                    continue;
                }
            }
        }
        printed += 1;
        if now == rule.default_stance {
            println!("{} [{}]", rule.id, now.as_str());
        } else {
            println!(
                "{} [{}, ships as {}]",
                rule.id,
                now.as_str(),
                rule.default_stance.as_str()
            );
        }
        println!("  {reason}");
        println!("  → {remedy}");
        println!(
            "  ▸ {}",
            backtest::excerpt(src, finding.span.start, finding.span.end)
        );
    }
    if printed == 0 {
        // A live rule's `examine` lets an unreadable command through to its
        // `confirm`; when nothing was confirmed, the verdict is the lexer's.
        match &parsed {
            shell::Parsed::Opaque(why) => println!("no opinion: {}", why.why()),
            _ => println!("no rule fires"),
        }
    }
    name_the_unread(src, &parsed);
    ExitCode::SUCCESS
}

/// Say which pipelines went unread, after the verdict.
///
/// The verdict stays on the first line so that "no rule fires" and "no
/// opinion" keep meaning what they meant. What follows is the difference
/// between having looked and having been able to look.
fn name_the_unread(src: &str, parsed: &shell::Parsed) {
    let clauses = parsed.clauses();
    for (i, cmd) in clauses.iter().enumerate() {
        let Some(why) = &cmd.opaque else { continue };
        // One line per unreadable PIPELINE: a clause whose predecessor pipes
        // into it is a later stage of a run already named.
        if cmd.prev.is_some_and(|c| c.is_pipe()) {
            continue;
        }
        let mut end = i;
        while clauses[end].next.is_some_and(|c| c.is_pipe()) && end + 1 < clauses.len() {
            end += 1;
        }
        println!("  ⋯ not read: {}", why.why());
        println!("    {}", backtest::excerpt(src, cmd.at, clauses[end].end));
    }
}

/// A stance as `rules` shows it: the one in force, and what the rule ships as
/// only when they differ — that difference is a decision somebody made on
/// this machine. Printing the shipped default alone listed a rule promoted to
/// `deny` as `observe`, so the one command you would run to ask whether a
/// rule is armed was the one that got it wrong.
fn stance_shown(now: rules::Stance, ships: rules::Stance) -> String {
    if now == ships {
        now.as_str().to_string()
    } else {
        format!("{} (ships as {})", now.as_str(), ships.as_str())
    }
}

const RULES_USAGE: &str = "usage: amont-agent rules [--json]\n";

/// `rules` takes `--json` and nothing else. It matches its arguments itself:
/// `flags()` accepts every flag any subcommand knows and keeps bare words,
/// so `rules --force` or `rules foo` would pass through it silently, and
/// until 2.27 `rules` ignored its arguments altogether — `rules --json`
/// printed the text table and exited 0.
fn run_rules(args: &[OsString]) -> ExitCode {
    let args: Vec<String> = args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    // Help wins wherever it appears, as in the other subcommands.
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{RULES_USAGE}");
        return ExitCode::SUCCESS;
    }
    match args.as_slice() {
        [] => print_rules_text(),
        [a] if a == "--json" => print_rules_json(),
        [a, rest @ ..] if a == "--json" => {
            eprintln!(
                "amont-agent: unexpected argument `{}`; `rules` takes only --json",
                rest[0]
            );
            ExitCode::from(2)
        }
        [a, ..] => {
            eprintln!("amont-agent: unknown argument `{a}`; `rules` takes only --json");
            ExitCode::from(2)
        }
    }
}

/// One object per rule, then per assertion, in the order the text prints
/// them. The keys are an interface — `scripts/bump-tap.py` and the tap's
/// formula read them — and `tests/rules_json.rs` pins the exact set.
/// `default_stance` is what ships; `stance` is what is in force here.
fn print_rules_json() -> ExitCode {
    let entry = |id: &str,
                 kind: &str,
                 ships: rules::Stance,
                 now: rules::Stance,
                 max: rules::Stance,
                 e: &rules::Evidence,
                 prevents: &str,
                 irreversible: bool| {
        json::object(&[
            json::string_field("id", id),
            json::string_field("kind", kind),
            json::string_field("default_stance", ships.as_str()),
            json::string_field("stance", now.as_str()),
            json::string_field("max_stance", max.as_str()),
            json::float_field("per_1000", e.per_1000),
            json::string_field("measured", e.measured),
            json::string_field("prevents", prevents),
            json::bool_field("irreversible", irreversible),
        ])
    };
    let mut items: Vec<String> = rule_table::RULES
        .iter()
        .map(|r| {
            entry(
                r.id,
                "rule",
                r.default_stance,
                stance::resolve(r),
                r.max_stance,
                &r.evidence,
                r.prevents,
                r.irreversible,
            )
        })
        .collect();
    // An assertion has no ceiling, so it reports `deny`: nothing caps it.
    items.extend(assertions::ASSERTIONS.iter().map(|a| {
        entry(
            a.id,
            "assertion",
            a.default_stance,
            stance::resolve_assertion(a),
            rules::Stance::Deny,
            &a.evidence,
            a.prevents,
            a.irreversible,
        )
    }));
    println!("{}", json::array(&items));
    ExitCode::SUCCESS
}

fn print_rules_text() -> ExitCode {
    let w = ui::id_column(rule_table::RULES);
    let rule_stances: Vec<String> = rule_table::RULES
        .iter()
        .map(|r| stance_shown(stance::resolve(r), r.default_stance))
        .collect();
    let assertion_stances: Vec<String> = assertions::ASSERTIONS
        .iter()
        .map(|a| stance_shown(stance::resolve_assertion(a), a.default_stance))
        .collect();
    // Wide enough for the longest entry, so a moved stance does not push its
    // row's evidence out of line with the others.
    let s = rule_stances
        .iter()
        .chain(&assertion_stances)
        .map(String::len)
        .max()
        .unwrap_or(0)
        .max(8);
    for (r, shown) in rule_table::RULES.iter().zip(&rule_stances) {
        // Named only when it bites: a rule capped below `deny` can never be
        // configured past it, and that is worth seeing next to its stance.
        let ceiling = if r.max_stance < rules::Stance::Deny {
            format!("  (max {})", r.max_stance.as_str())
        } else {
            String::new()
        };
        println!(
            "{:<w$}{:<s$} {:>6.1}/1000  measured {}{ceiling}",
            r.id, shown, r.evidence.per_1000, r.evidence.measured
        );
    }
    // Listed apart, because they answer a different question: a rule judges a
    // command before it runs, an assertion checks what a command that already
    // reported success actually did.
    for (a, shown) in assertions::ASSERTIONS.iter().zip(&assertion_stances) {
        println!(
            "{:<w$}{:<s$} {:>6.1}/1000  measured {}  (assertion)",
            a.id, shown, a.evidence.per_1000, a.evidence.measured
        );
    }
    ExitCode::SUCCESS
}

fn run_install(args: &[OsString], adding: bool) -> ExitCode {
    let f = match flags(args) {
        Ok(f) => f,
        Err(why) => {
            eprintln!("amont-agent: {why}");
            return ExitCode::from(2);
        }
    };
    let scope = if f.local {
        settings::Scope::ProjectLocal
    } else if f.project {
        settings::Scope::Project
    } else {
        settings::Scope::User
    };
    let project = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let Some(path) = scope.path(&project) else {
        eprintln!("amont-agent: cannot find your Claude Code settings directory");
        return ExitCode::from(2);
    };
    // The absolute path of THIS binary. A `PATH`-resolved command exits 127
    // into Claude Code's non-blocking bucket the moment PATH differs, which
    // disables the guard with nothing to notice it.
    let bin = match std::env::current_exe() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("amont-agent: cannot resolve my own path: {e}");
            return ExitCode::from(2);
        }
    };

    let planned = if adding {
        settings::plan_install(&path, &bin, f.reformat)
    } else {
        settings::plan_uninstall(&path, f.reformat)
    };
    let plan = match planned {
        Ok(p) => p,
        Err(e) => {
            eprintln!("amont-agent: {}", e.explain());
            if adding {
                eprintln!("\n{}", settings::snippet(&bin));
            }
            return ExitCode::from(2);
        }
    };

    if !f.write {
        if matches!(plan.change, settings::Change::RemoveFile) {
            println!(
                "{} would be removed: nothing else is left in it.",
                plan.path.display()
            );
            println!("Nothing written. Re-run with --write to apply.");
            return ExitCode::SUCCESS;
        }
        println!("{}", plan.change.describe(&plan.path));
        println!("Nothing written. Re-run with --write to apply:\n");
        print!("{}", plan.after);
        return ExitCode::SUCCESS;
    }
    if matches!(plan.change, settings::Change::WouldReformat) {
        eprintln!("{}\n", plan.change.describe(&plan.path));
        eprint!("{}", settings::snippet(&bin));
        return ExitCode::from(2);
    }
    match settings::apply(&plan) {
        Ok(()) => {
            println!("{}", plan.change.describe(&plan.path));
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("amont-agent: could not write {}: {e}", plan.path.display());
            ExitCode::from(2)
        }
    }
}

/// How far back `status` reads the journal. Long enough for a rule that
/// fires a few times a week to show a shape; short enough that a rule fixed a
/// month ago stops being judged on what it did before.
const SEEN_WINDOW: u64 = 30 * 24 * 60 * 60;

/// The plan root in use and where it came from.
fn plan_root_finding() -> doctor::Finding {
    use plan_review::{RootSource, ROOT_REMEDY};
    match plan_review::resolved_root() {
        Some((path, RootSource::CwdFallback)) => doctor::Finding::warn(
            format!("plan root: {} (cwd fallback)", path.display()),
            format!("plan review falls back to the working directory and the attestations sweep removes nothing; {ROOT_REMEDY}"),
        ),
        Some((path, source)) => doctor::Finding::good(format!(
            "plan root: {} ({})",
            path.display(),
            source.label()
        )),
        None => doctor::Finding::warn(
            "plan root: unset",
            format!("plan review cannot resolve repos= and the attestations sweep removes nothing; {ROOT_REMEDY}"),
        ),
    }
}

fn run_status() -> ExitCode {
    let w = ui::id_column(rule_table::RULES);
    let seen = journal::tally(SEEN_WINDOW);
    println!(
        "{:<w$}{:<10}{:<10}  {:<31}  seen, last 30 days",
        "rule", "ships as", "now", "evidence"
    );
    for r in rule_table::RULES {
        let now = stance::resolve(r);
        println!(
            "{:<w$}{:<10}{:<10}  {:<31}  {}",
            r.id,
            r.default_stance.as_str(),
            now.as_str(),
            format!(
                "{:.1}/1000 measured {}",
                r.evidence.per_1000, r.evidence.measured
            ),
            describe_seen(seen.get(r.id))
        );
    }
    if let Some(p) = journal::path() {
        println!("\njournal: {}", p.display());
    }
    ExitCode::SUCCESS
}

/// One cell of the `seen` column.
///
/// The counts are what happened; the reason is why it did not. A rule whose
/// `confirm` declined a hundred times for `already in a linked worktree` is a
/// rule watching a convention being followed, and that is the sentence that
/// tells a reader whether the rule is precise or merely quiet.
fn describe_seen(seen: Option<&journal::Seen>) -> String {
    let Some(s) = seen.filter(|s| s.total() > 0) else {
        return "-".to_string();
    };
    let mut parts = Vec::new();
    for (n, what) in [
        (s.denied, "denied"),
        (s.advised, "advised"),
        (s.watched, "watched"),
    ] {
        if n > 0 {
            parts.push(format!("{n} {what}"));
        }
    }
    if s.unconfirmed > 0 {
        match s.top_reason() {
            // Records written before the reason was carried say `skipped`,
            // which is not a reason. Show the count and nothing invented.
            Some((why, n)) if why != "skipped" => parts.push(format!(
                "{} unconfirmed ({} \u{d7}{n})",
                s.unconfirmed,
                why.replace('_', " ")
            )),
            _ => parts.push(format!("{} unconfirmed", s.unconfirmed)),
        }
    }
    parts.join(", ")
}

fn run_corpus(args: &[OsString]) -> ExitCode {
    let f = match flags(args) {
        Ok(f) => f,
        Err(why) => {
            eprintln!("amont-agent: {why}");
            return ExitCode::from(2);
        }
    };
    match f.rest.first().map(String::as_str) {
        Some("check") | None => {}
        Some(other) => {
            eprintln!("amont-agent: corpus takes `check`, not `{other}`");
            return ExitCode::from(2);
        }
    }
    let mut healthy = true;
    let w = ui::id_column(rule_table::RULES);
    for rule in rule_table::RULES {
        let score = corpus::score(rule);
        if score.reviewed == 0 && score.unreviewed == 0 {
            println!("{:<w$}no cases yet", rule.id);
            continue;
        }
        let precision = match score.precision() {
            Some(p) => format!("{:.0}%", p * 100.0),
            None => "unmeasured".to_string(),
        };
        println!(
            "{:<w$}{} reviewed ({} negative), {} unreviewed, precision {precision}",
            rule.id, score.reviewed, score.negatives, score.unreviewed
        );
        for d in &score.disagreements {
            healthy = false;
            // The checkout's file when there is one; outside a checkout the
            // path the binary was built from means nothing on this disk.
            let path = corpus::path_for(rule.id);
            let shown = if path.exists() {
                path.display().to_string()
            } else {
                format!("{}.cases", rule.id)
            };
            println!(
                "  {}:{} expected {} — {}",
                shown,
                d.line,
                d.expected.as_str(),
                corpus::escape(&d.command)
            );
        }
    }
    if healthy {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn run_graduate(args: &[OsString], promoting: bool) -> ExitCode {
    let f = match flags(args) {
        Ok(f) => f,
        Err(why) => {
            eprintln!("amont-agent: {why}");
            return ExitCode::from(2);
        }
    };
    let Some(id) = f.rest.first() else {
        eprintln!("amont-agent: name a rule");
        return ExitCode::from(2);
    };
    let Some(rule) = rules::by_id(rule_table::RULES, id) else {
        let known: Vec<&str> = rule_table::RULES.iter().map(|r| r.id).collect();
        eprintln!("amont-agent: no rule `{id}` — known: {}", known.join(", "));
        return ExitCode::from(2);
    };
    let to = if promoting {
        let Some(to) = f.to.as_deref().and_then(rules::Stance::parse) else {
            eprintln!("amont-agent: graduate needs --to advise|deny");
            return ExitCode::from(2);
        };
        to
    } else {
        rules::Stance::Observe
    };

    let verdict = graduate::assess(rule, to);
    for (ok, line) in &verdict.lines {
        println!(
            "  {} {line}",
            if *ok {
                crate::ui::valid_sign()
            } else {
                crate::ui::error_sign()
            }
        );
    }
    if !verdict.allowed && !f.force {
        // The file to append to is the checkout's. An installed copy has no
        // checkout, and naming the path of the machine that built it would
        // send the reader to a directory that exists nowhere near them.
        let where_ = if corpus::checkout_present() {
            corpus::path_for(rule.id).display().to_string()
        } else {
            format!(
                "crates/amont-agent-guard/tests/corpus/{}.cases   # in a checkout of amont-agent",
                rule.id
            )
        };
        eprintln!(
            "\nrefusing to move {} to {}: not enough evidence.\n\
             Review its matches first:\n  \
             amont-agent explain {} --format cases >> {}\n\
             then label each line and re-run. `--force` overrides and is recorded as forced.",
            rule.id,
            to.as_str(),
            rule.id,
            where_
        );
        return ExitCode::from(2);
    }
    match graduate::set(rule, to) {
        Ok(()) => {
            println!(
                "{} is now `{}`{}",
                rule.id,
                to.as_str(),
                if verdict.allowed { "" } else { " (forced)" }
            );
            ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("amont-agent: could not record the stance: {why}");
            ExitCode::from(2)
        }
    }
}
