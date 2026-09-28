//! What known network clients do with their arguments.
//!
//! Each model turns one invocation — the program and its arguments after
//! shell expansion, as abstract values — into the explicit command-line
//! transfers it makes: one [`Transfer`] per recognised destination. Anything
//! a model cannot establish stays explicit: an option it does not know makes
//! every later operand [`Unresolved::UnknownOption`], never a guessed URL.
//!
//! The option parser here is shared: every model declares its option table
//! and gets back a token stream in which values have already been paired
//! with their options. That pairing is the whole point — `-H 'Referer:
//! https://b/'` must never be read as a destination — so it is done once,
//! with one set of rules about what an argument that might be an option
//! does to the operands after it.

mod curl;
mod gh;
mod httpie;
mod misc;
mod registry;
mod wget;

use super::domain::Count;
use super::effects::{Auth, Factor, TargetSet, Unresolved};
use super::ir::Span;
use super::state::{is_local_host, url_host, AbsVal};

/// One argument after expansion.
#[derive(Debug, Clone)]
pub struct Arg {
    /// Its value. When `fields` is more than one (an unquoted brace range,
    /// a split parameter), this is the join of the fields' values.
    pub val: AbsVal,
    /// How many shell words this argument expanded to.
    pub fields: Count,
    /// Where it was written.
    pub span: Span,
}

/// One destination one invocation reaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transfer {
    pub program: &'static str,
    /// Transfers to this destination per invocation.
    pub per_call: Count,
    pub targets: TargetSet,
    pub auth: Auth,
    /// Requests the client may make that are not counted.
    pub uncounted: Vec<&'static str>,
    /// Multipliers inside the invocation (`--retry`, a URL range), in order.
    pub builtin: Vec<Factor>,
    /// The operand (or the invocation) this transfer came from.
    pub span: Span,
}

/// The programs a model exists for — what the hook's prefilter looks for.
pub const PROGRAMS: &[&str] = &[
    "curl", "wget", "gh", "http", "https", "skopeo", "crane", "regctl", "oras", "docker", "podman",
    "npm", "git",
];

/// The transfers `program args…` makes, or `None` when `program` is not a
/// network client the analysis models. `span` is the whole invocation.
pub fn model(program: &str, args: &[Arg], span: Span) -> Option<Vec<Transfer>> {
    // `/usr/bin/curl` runs curl; the directory says nothing about behaviour.
    let name = program.rsplit('/').next().unwrap_or(program);
    match name {
        "curl" => Some(curl::model(args)),
        "wget" => Some(wget::model(args)),
        "gh" => gh::model(args, span),
        "http" => Some(httpie::model("http", args)),
        "https" => Some(httpie::model("https", args)),
        "skopeo" | "crane" | "regctl" | "oras" | "docker" | "podman" => registry::model(name, args),
        "npm" => misc::npm(args, span),
        "git" => misc::git(args, span),
        _ => None,
    }
}

/// Where a URL-valued argument points.
///
/// Each exact value names its host when it has one; a known prefix names it
/// only when the authority is complete inside the prefix (`url_host` owns
/// that rule). A value that names no host — nothing known, or nothing
/// host-shaped — is [`Unresolved::Dynamic`]: the analysis never guesses.
pub fn target_of(val: &AbsVal) -> TargetSet {
    match val {
        AbsVal::Values(vs) => vs
            .iter()
            .map(|v| host_target(url_host(v, true)))
            .reduce(|a, b| a.union(&b))
            .unwrap_or_else(|| TargetSet::unresolved(Unresolved::Dynamic)),
        AbsVal::Prefix(p) => host_target(url_host(p, false)),
        AbsVal::Top | AbsVal::Token => TargetSet::unresolved(Unresolved::Dynamic),
    }
}

/// A host (or its absence) as a target.
fn host_target(host: Option<String>) -> TargetSet {
    match host {
        None => TargetSet::unresolved(Unresolved::Dynamic),
        Some(h) if is_local_host(&h) => TargetSet::local(),
        Some(h) => TargetSet::host(h),
    }
}

/// A transfer with nothing but its essentials; models add notes and
/// factors to it.
fn transfer(program: &'static str, per_call: Count, targets: TargetSet, span: Span) -> Transfer {
    Transfer {
        program,
        per_call,
        targets,
        auth: Auth::Other,
        uncounted: Vec::new(),
        builtin: Vec::new(),
        span,
    }
}

/// Where an operand points, unless an option the analysis could not read
/// came before it — then the operand may have been that option's value.
fn operand_target(val: &AbsVal, after_unknown: bool) -> TargetSet {
    if after_unknown {
        TargetSet::unresolved(Unresolved::UnknownOption)
    } else {
        target_of(val)
    }
}

/// How many words an argument that may not be an operand at all contributes
/// as operands: none established, at most its fields.
fn maybe_fields(fields: Count) -> Count {
    Count::MAYBE.mul(fields)
}

/// The integer an option's value spells, when it is exact.
fn exact_u64(val: &AbsVal) -> Option<u64> {
    val.as_exact().and_then(|s| s.trim().parse().ok())
}

/// One option in a program's table.
#[derive(Debug, Clone, Copy)]
struct OptSpec {
    /// The long name without dashes; also the canonical name models match
    /// on, so `-H` and `--header` are one option to them.
    long: &'static str,
    short: Option<char>,
    value: bool,
}

const fn flag(long: &'static str) -> OptSpec {
    OptSpec {
        long,
        short: None,
        value: false,
    }
}

const fn flag_c(short: char, long: &'static str) -> OptSpec {
    OptSpec {
        long,
        short: Some(short),
        value: false,
    }
}

const fn val(long: &'static str) -> OptSpec {
    OptSpec {
        long,
        short: None,
        value: true,
    }
}

const fn val_c(short: char, long: &'static str) -> OptSpec {
    OptSpec {
        long,
        short: Some(short),
        value: true,
    }
}

/// A program's option grammar.
struct Syntax {
    opts: &'static [OptSpec],
    /// `--no-<flag>` turns a flag off (curl, wget, httpie, npm, git).
    negatable: bool,
    /// Go flag parsers accept `--flag=false` on a boolean.
    go_bools: bool,
}

impl Syntax {
    fn long(&self, name: &str) -> Option<&'static OptSpec> {
        self.opts.iter().find(|o| o.long == name)
    }

    fn short(&self, c: char) -> Option<&'static OptSpec> {
        self.opts.iter().find(|o| o.short == Some(c))
    }
}

/// One parsed element of an invocation.
#[derive(Debug)]
enum Tok<'a> {
    Flag {
        name: &'static str,
        on: bool,
    },
    /// A value-taking option with its value. `after_unknown`: an option the
    /// analysis could not read came earlier, so this may have been its value.
    Value {
        name: &'static str,
        val: AbsVal,
        span: Span,
        after_unknown: bool,
    },
    Operand {
        arg: &'a Arg,
        after_unknown: bool,
    },
    /// An argument whose value is not known well enough to tell an option
    /// from an operand. It may be either; everything after it is read as
    /// if after an unknown option.
    Opaque {
        arg: &'a Arg,
        after_unknown: bool,
    },
    /// An option the table does not have.
    Unknown,
}

/// What an argument's value says about its role.
enum Shape<'v> {
    Word(&'v str),
    Operand,
    Opaque,
    /// `--name=` followed by something unknown: the option is known even
    /// though its value is not.
    LongPrefix(&'v str, &'v str),
}

fn shape(arg: &Arg) -> Shape<'_> {
    let dashed = |s: &str| s.len() > 1 && s.starts_with('-');
    match &arg.val {
        AbsVal::Values(vs) => {
            if let Some(s) = arg.val.as_exact() {
                // `$flags` split into several words: which options, in which
                // order, is not something the joined value records.
                if dashed(s) && arg.fields != Count::ONE {
                    Shape::Opaque
                } else {
                    Shape::Word(s)
                }
            } else if vs.iter().any(|s| dashed(s)) {
                Shape::Opaque
            } else {
                Shape::Operand
            }
        }
        AbsVal::Prefix(p) if dashed(p) => {
            match p.strip_prefix("--").and_then(|b| b.split_once('=')) {
                Some((name, rest)) => Shape::LongPrefix(name, rest),
                None => Shape::Opaque,
            }
        }
        AbsVal::Prefix(_) => Shape::Operand,
        AbsVal::Top | AbsVal::Token => Shape::Opaque,
    }
}

/// Tokenise `args` against `syn`.
///
/// Every argument that could be an option the analysis cannot read — an
/// unknown option name, a value not known to be an operand — sets
/// `after_unknown` on everything after it: the unknown option may have
/// taken the next argument as its value, so no later operand is
/// established as a destination.
fn parse<'a>(args: &'a [Arg], syn: &Syntax) -> Vec<Tok<'a>> {
    let mut out = Vec::new();
    let mut poisoned = false;
    let mut ended = false;
    let mut i = 0;
    // The next argument as an option's value. Unless it is exactly one
    // word, which words the option took and which remain is not known.
    let take_next = |i: &mut usize, poisoned: &mut bool| -> Option<(AbsVal, Span)> {
        let next = args.get(*i)?;
        *i += 1;
        if next.fields != Count::ONE {
            *poisoned = true;
        }
        Some((next.val.clone(), next.span.clone()))
    };
    while i < args.len() {
        let arg = &args[i];
        i += 1;
        if arg.fields.is_zero() {
            continue;
        }
        if ended {
            out.push(Tok::Operand {
                arg,
                after_unknown: poisoned,
            });
            continue;
        }
        match shape(arg) {
            Shape::Operand => out.push(Tok::Operand {
                arg,
                after_unknown: poisoned,
            }),
            Shape::Opaque => {
                out.push(Tok::Opaque {
                    arg,
                    after_unknown: poisoned,
                });
                poisoned = true;
            }
            Shape::LongPrefix(name, rest) => match syn.long(name) {
                Some(o) if o.value => out.push(Tok::Value {
                    name: o.long,
                    val: if rest.is_empty() {
                        AbsVal::Top
                    } else {
                        AbsVal::Prefix(rest.to_string())
                    },
                    span: arg.span.clone(),
                    after_unknown: poisoned,
                }),
                _ => {
                    out.push(Tok::Opaque {
                        arg,
                        after_unknown: poisoned,
                    });
                    poisoned = true;
                }
            },
            Shape::Word("--") => ended = true,
            Shape::Word(s) if s.starts_with("--") => {
                let body = &s[2..];
                let (name, inline) = match body.split_once('=') {
                    Some((n, v)) => (n, Some(v)),
                    None => (body, None),
                };
                let found = syn.long(name).map(|o| (o, true)).or_else(|| {
                    name.strip_prefix("no-")
                        .filter(|_| syn.negatable)
                        .and_then(|n| syn.long(n))
                        .filter(|o| !o.value)
                        .map(|o| (o, false))
                });
                match (found, inline) {
                    (Some((o, _)), inline) if o.value => {
                        let v = match inline {
                            Some(v) => Some((AbsVal::exact(v), arg.span.clone())),
                            None => take_next(&mut i, &mut poisoned),
                        };
                        if let Some((val, span)) = v {
                            out.push(Tok::Value {
                                name: o.long,
                                val,
                                span,
                                after_unknown: poisoned,
                            });
                        }
                    }
                    (Some((o, on)), None) => out.push(Tok::Flag { name: o.long, on }),
                    (Some((o, on)), Some(v)) if syn.go_bools => out.push(Tok::Flag {
                        name: o.long,
                        on: on && v != "false",
                    }),
                    _ => {
                        out.push(Tok::Unknown);
                        poisoned = true;
                    }
                }
            }
            Shape::Word(s) if s.len() > 1 && s.starts_with('-') => {
                for (at, c) in s.char_indices().skip(1) {
                    let Some(o) = syn.short(c) else {
                        out.push(Tok::Unknown);
                        poisoned = true;
                        break;
                    };
                    if !o.value {
                        out.push(Tok::Flag {
                            name: o.long,
                            on: true,
                        });
                        continue;
                    }
                    // A value-taking letter takes the rest of the cluster,
                    // or the next argument when the cluster ends with it.
                    let mut rest = &s[at + c.len_utf8()..];
                    if syn.go_bools {
                        rest = rest.strip_prefix('=').unwrap_or(rest);
                    }
                    let v = if rest.is_empty() {
                        take_next(&mut i, &mut poisoned)
                    } else {
                        Some((AbsVal::exact(rest), arg.span.clone()))
                    };
                    if let Some((val, span)) = v {
                        out.push(Tok::Value {
                            name: o.long,
                            val,
                            span,
                            after_unknown: poisoned,
                        });
                    }
                    break;
                }
            }
            Shape::Word(_) => out.push(Tok::Operand {
                arg,
                after_unknown: poisoned,
            }),
        }
    }
    out
}

/// The operands of a token stream, with whether each may be an option.
fn operands<'a>(toks: &[Tok<'a>]) -> Vec<(&'a Arg, bool, bool)> {
    toks.iter()
        .filter_map(|t| match t {
            Tok::Operand { arg, after_unknown } => Some((*arg, *after_unknown, false)),
            Tok::Opaque { arg, after_unknown } => Some((*arg, *after_unknown, true)),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod testutil {
    use super::*;

    /// An exact single-word argument.
    pub fn a(s: &str) -> Arg {
        Arg {
            val: AbsVal::exact(s),
            fields: Count::ONE,
            span: 0..s.len(),
        }
    }

    pub fn run(program: &str, args: &[&str]) -> Option<Vec<Transfer>> {
        let args: Vec<Arg> = args.iter().map(|s| a(s)).collect();
        model(program, &args, 0..1)
    }

    pub fn only(program: &str, args: &[&str]) -> Transfer {
        let mut t = run(program, args).expect("modelled");
        assert_eq!(t.len(), 1, "{t:?}");
        t.remove(0)
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;

    #[test]
    fn a_set_of_exact_urls_targets_every_host() {
        let v = AbsVal::Values(
            ["https://a.example/x", "https://b.example/y"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        );
        let t = model(
            "curl",
            &[Arg {
                val: v,
                fields: Count::ONE,
                span: 0..1,
            }],
            0..1,
        )
        .unwrap();
        assert_eq!(t.len(), 1);
        let hosts: Vec<&str> = t[0].targets.hosts.iter().map(String::as_str).collect();
        assert_eq!(hosts, ["a.example", "b.example"]);
    }

    #[test]
    fn a_prefix_names_a_host_only_once_its_authority_is_complete() {
        assert_eq!(
            target_of(&AbsVal::Prefix("https://ghcr.io".into())),
            TargetSet::unresolved(Unresolved::Dynamic)
        );
        assert_eq!(
            target_of(&AbsVal::Prefix("https://ghcr.io/v2/".into())),
            TargetSet::host("ghcr.io")
        );
    }

    #[test]
    fn an_argument_that_expanded_to_several_words_is_that_many_transfers() {
        let urls = AbsVal::Values((1..=3).map(|i| format!("https://a.example/{i}")).collect());
        let t = model(
            "curl",
            &[Arg {
                val: urls,
                fields: Count::exactly(3),
                span: 0..1,
            }],
            0..1,
        )
        .unwrap();
        assert_eq!(t[0].per_call, Count::exactly(3));
        assert_eq!(t[0].targets, TargetSet::host("a.example"));
    }

    #[test]
    fn an_unknown_word_may_be_an_option_so_it_hides_what_follows() {
        let args = [
            Arg {
                val: AbsVal::Top,
                fields: Count::UNKNOWN,
                span: 0..1,
            },
            a("https://a.example/"),
        ];
        let t = model("curl", &args, 0..1).unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].per_call.lower, 0, "it may not be an operand at all");
        assert_eq!(
            t[1].targets,
            TargetSet::unresolved(Unresolved::UnknownOption)
        );
    }

    #[test]
    fn a_directory_does_not_hide_the_program() {
        assert_eq!(
            only("/usr/bin/curl", &["https://a.example/x"]).targets,
            TargetSet::host("a.example")
        );
    }

    #[test]
    fn unmodelled_programs_are_none() {
        assert!(run("ls", &["-l"]).is_none());
    }

    #[test]
    fn every_program_listed_is_dispatched() {
        for p in PROGRAMS {
            // Not every program is modelled for every subcommand, but none
            // listed may fall through to "not a network client" entirely.
            let probe: &[&str] = match *p {
                "gh" => &["api", "x"],
                "skopeo" => &["inspect", "docker://a.example/x"],
                "crane" => &["manifest", "x"],
                "regctl" => &["manifest", "get", "x"],
                "oras" => &["pull", "a.example/x"],
                "docker" | "podman" => &["pull", "x"],
                "npm" => &["view", "x"],
                "git" => &["ls-remote", "x"],
                _ => &["https://a.example/"],
            };
            assert!(run(p, probe).is_some(), "{p}");
        }
    }
}
