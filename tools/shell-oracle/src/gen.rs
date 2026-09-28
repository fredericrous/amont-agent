//! A seeded generator of small, bounded shell scripts in the analyzer's
//! supported subset (docs/analysis.md), written to be valid — and to
//! terminate — under both bash and zsh.
//!
//! No RNG crate: a xorshift64* is plenty, and a seed printed on failure
//! reproduces the script on any machine.

/// xorshift64*.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        // Never zero; scramble small seeds so 1, 2, 3 diverge at once.
        let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        for _ in 0..4 {
            r.next_u64();
        }
        r
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform in `lo..=hi`.
    pub fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next_u64() % (hi - lo + 1)
    }
    /// True with probability `num/den`.
    pub fn chance(&mut self, num: u64, den: u64) -> bool {
        self.next_u64() % den < num
    }
    pub fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[(self.next_u64() % xs.len() as u64) as usize]
    }
}

const HOSTS: &[&str] = &["a.example", "b.example", "c.example"];
const WORDS: &[&str] = &["a", "b", "c", "x"];
const MAX_DEPTH: usize = 3;
/// The product of the trip counts of the loops enclosing a statement stays
/// under this. A stub call costs ~6 ms on macOS (fork + exec), so this is
/// what keeps a script — functions called three times included — well
/// inside the harness's wall-clock limit.
const MAX_WORK: u64 = 48;
/// Estimated transfers per script, which call sites may not push it past.
const MAX_COST: u64 = 300;

#[derive(Clone)]
struct Loop {
    /// Counter-pattern loop whose increment sits at the END of the body: a
    /// `continue` straight inside it would skip the increment forever.
    counter_at_end: bool,
    /// The loop's variable, and whether it holds a word (for) or a number.
    var: String,
    numeric: bool,
}

struct Gen {
    rng: Rng,
    hosts: Vec<&'static str>,
    /// Defined functions with their estimated cost (transfers per call).
    funcs: Vec<(String, u64)>,
    /// Estimated transfers so far in the body being generated: each transfer
    /// site adds the enclosing trip-count product, each call its callee's
    /// cost times that product. It gates call sites, which the loop bound
    /// alone cannot see.
    cost: u64,
    loops: Vec<Loop>,
    in_func: bool,
    /// Inside `( )` with no loop opened since: a `break`/`continue` here
    /// names a loop of the PARENT shell.
    in_sub: bool,
    opts: Options,
    work: u64,
    next_var: usize,
    set_e: bool,
}

/// What the generator may produce.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// Leave out the constructs behind the analyzer bugs already reported
    /// (see tests/execution.rs, `known_bug_*`), so a sweep can look for NEW
    /// ones past them. Never the default.
    pub avoid_known_bugs: bool,
}

/// Generate the script for `seed`.
pub fn script(seed: u64) -> String {
    script_with(seed, Options::default())
}

/// Generate the script for `seed` under `opts`. The same seed makes the same
/// script only under the same options.
pub fn script_with(seed: u64, opts: Options) -> String {
    let mut rng = Rng::new(seed);
    let nh = rng.range(2, 3) as usize;
    let hosts = HOSTS[..nh].to_vec();
    let mut g = Gen {
        rng,
        hosts,
        funcs: Vec::new(),
        cost: 0,
        loops: Vec::new(),
        in_func: false,
        in_sub: false,
        opts,
        work: 1,
        next_var: 0,
        set_e: false,
    };
    let mut parts = Vec::new();
    if g.rng.chance(1, 4) {
        g.set_e = true;
        parts.push("set -e".to_string());
    }
    if g.rng.chance(1, 3) {
        parts.push(format!("k={}", g.rng.pick(WORDS)));
    }
    if g.rng.chance(1, 2) {
        parts.push(r#"items="a b c""#.to_string());
    }
    let nf = g.rng.range(0, 2);
    for n in 0..nf {
        let name = format!("f{n}");
        g.in_func = true;
        g.cost = 0;
        let body = g.block(1);
        g.in_func = false;
        parts.push(format!("{name}() {{ {body}; }}"));
        g.funcs.push((name, g.cost.max(1)));
        g.cost = 0;
    }
    let n = g.rng.range(1, 4);
    for _ in 0..n {
        parts.push(g.stmt(0));
    }
    // Make sure every defined function has a chance to run.
    for (f, c) in g.funcs.clone() {
        if g.rng.chance(1, 2) {
            let times = g.rng.range(0, 3);
            for _ in 0..times {
                if g.cost + c <= MAX_COST {
                    g.cost += c;
                    parts.push(f.clone());
                }
            }
        }
    }
    parts.join("; ")
}

impl Gen {
    fn host(&mut self) -> &'static str {
        let i = (self.rng.next_u64() % self.hosts.len() as u64) as usize;
        self.hosts[i]
    }

    fn fresh(&mut self, stem: &str) -> String {
        self.next_var += 1;
        format!("{stem}{}", self.next_var)
    }

    fn transfer(&mut self) -> String {
        self.cost += self.work;
        let h = self.host();
        let p = self.rng.range(1, 9);
        match self.rng.range(0, 9) {
            0..=3 => format!("curl -s https://{h}/p{p}"),
            4 | 5 => format!("wget -q https://{h}/x"),
            6 => "gh api /x".to_string(),
            7 => format!("v=$(curl -s https://{h}/p{p})"),
            _ => format!("curl -s https://{h}/p{p} | read v"),
        }
    }

    /// A condition the analyzer may or may not be able to decide.
    fn cond(&mut self) -> String {
        let mut opts: Vec<String> = vec![
            "true".into(),
            "false".into(),
            r#"[ -n "$X" ]"#.into(),
            r#"[ -z "$X" ]"#.into(),
            format!(r#"[ "$k" = {} ]"#, self.rng.pick(WORDS)),
            format!(r#"[ "$v" = {} ]"#, self.rng.pick(WORDS)),
            format!("curl -s https://{}/c", self.host()),
        ];
        if let Some(l) = self.loops.last().cloned() {
            if l.numeric {
                let n = self.rng.range(0, 6);
                opts.push(format!("[ ${} -eq {n} ]", l.var));
                opts.push(format!("[ ${} -gt {n} ]", l.var));
            } else {
                opts.push(format!(r#"[ "${}" = {} ]"#, l.var, self.rng.pick(WORDS)));
            }
        }
        self.rng.pick(&opts).clone()
    }

    fn block(&mut self, depth: usize) -> String {
        let n = self.rng.range(1, 3);
        (0..n)
            .map(|_| self.stmt(depth))
            .collect::<Vec<_>>()
            .join("; ")
    }

    fn stmt(&mut self, depth: usize) -> String {
        let can_nest = depth < MAX_DEPTH;
        loop {
            let pick = self.rng.range(0, 19);
            let s = match pick {
                0..=4 => Some(self.transfer()),
                5 | 6 if can_nest => self.for_loop(depth),
                7 | 8 if can_nest => self.counter_loop(depth),
                9 if can_nest => Some(self.if_stmt(depth)),
                10 if can_nest => Some(self.case_stmt(depth)),
                11 => {
                    let c = self.cond();
                    let t = self.transfer();
                    let op = if self.rng.chance(1, 2) { "&&" } else { "||" };
                    Some(format!("{c} {op} {t}"))
                }
                12 if can_nest => {
                    let saved = std::mem::replace(&mut self.in_sub, true);
                    let b = self.block(depth + 1);
                    self.in_sub = saved;
                    let exit = if self.rng.chance(1, 4) {
                        "; exit 0"
                    } else {
                        ""
                    };
                    let after = if exit.is_empty() {
                        String::new()
                    } else {
                        format!("; {}", self.transfer())
                    };
                    Some(format!("( {b}{exit}{after} )"))
                }
                13 => Some(format!("echo {} | read v", self.rng.pick(WORDS))),
                14 if !self.funcs.is_empty() => {
                    let (f, c) = self.rng.pick(&self.funcs.clone()).clone();
                    let add = c.saturating_mul(self.work);
                    (self.cost + add <= MAX_COST).then(|| {
                        self.cost += add;
                        f
                    })
                }
                15 if !self.loops.is_empty() => self.jump(),
                16 if self.in_func => {
                    let c = self.cond();
                    Some(if self.rng.chance(1, 2) {
                        format!("{c} && return 0")
                    } else {
                        format!("return 0; {}", self.transfer())
                    })
                }
                17 if self.in_func => {
                    // A write to a counter name: `local` (the caller's copy
                    // is safe) or plain (dynamic scope: it resets the
                    // caller's counter if one is live).
                    // Never a counter of a loop this function itself is
                    // inside: that is an infinite loop with no transfer to
                    // stop it at, only the clock. Resetting a CALLER's
                    // counter is the point.
                    let v = format!("i{}", self.rng.range(1, 3));
                    if self.loops.iter().any(|l| l.var == v) {
                        continue;
                    }
                    Some(if self.rng.chance(1, 2) {
                        format!("local {v}=0")
                    } else {
                        format!("{v}=0")
                    })
                }
                18 if self.set_e => Some("false".to_string()),
                19 => Some(format!("k={}", self.rng.pick(WORDS))),
                _ => None,
            };
            if let Some(s) = s {
                return s;
            }
        }
    }

    fn jump(&mut self) -> Option<String> {
        let l = self.loops.last()?.clone();
        if self.in_sub && self.opts.avoid_known_bugs {
            return None;
        }
        let c = self.cond();
        if self.rng.chance(1, 2) {
            Some(format!("{c} && break"))
        } else if !l.counter_at_end {
            Some(format!("{c} && continue"))
        } else {
            None
        }
    }

    fn trips(&mut self) -> Option<u64> {
        let cap = (MAX_WORK / self.work).min(12);
        if cap == 0 {
            return None;
        }
        Some(self.rng.range(0, cap))
    }

    fn with_loop<F: FnOnce(&mut Gen) -> String>(&mut self, l: Loop, trips: u64, f: F) -> String {
        let saved = self.work;
        self.work = self.work.saturating_mul(trips.max(1));
        self.loops.push(l);
        let sub = std::mem::replace(&mut self.in_sub, false);
        let s = f(self);
        self.in_sub = sub;
        self.loops.pop();
        self.work = saved;
        s
    }

    fn for_loop(&mut self, depth: usize) -> Option<String> {
        let n = self.trips()?;
        let var = self.fresh("x");
        let (header, numeric) = match self.rng.range(0, 3) {
            0 => {
                let n = n.min(4);
                let words: Vec<&str> = (0..n).map(|_| *self.rng.pick(WORDS)).collect();
                (format!("for {var} in {}", words.join(" ")), false)
            }
            1 => {
                let a = self.rng.range(0, 3);
                let b = a + n.saturating_sub(1);
                let (a, b) = if self.rng.chance(1, 4) {
                    (b, a)
                } else {
                    (a, b)
                };
                (format!("for {var} in {{{a}..{b}}}"), true)
            }
            2 => {
                // Never `seq a b` with b < a: GNU prints nothing, BSD counts
                // DOWN — the answer depends on whose seq is on PATH.
                let a = self.rng.range(1, 3);
                let b = a + n.max(1) - 1;
                (format!("for {var} in $(seq {a} {b})"), true)
            }
            _ if self.opts.avoid_known_bugs => (format!("for {var} in a b"), false),
            _ => (format!("for {var} in $items"), false),
        };
        let l = Loop {
            counter_at_end: false,
            var: var.clone(),
            numeric,
        };
        let body = self.with_loop(l, n, |g| g.block(depth + 1));
        Some(format!("{header}; do {body}; done"))
    }

    fn counter_loop(&mut self, depth: usize) -> Option<String> {
        let n = self.trips()?;
        // Counter names are shared with what functions may reset: i1..i3.
        let var = format!("i{}", (depth + 1).min(3));
        let init = self.rng.range(0, 2);
        let (cmp, bound) = match self.rng.range(0, 2) {
            0 => ("while", format!("[ ${var} -lt {} ]", init + n)),
            1 => (
                "while",
                format!("[ ${var} -le {} ]", (init + n).saturating_sub(1)),
            ),
            _ => ("until", format!("[ ${var} -ge {} ]", init + n)),
        };
        // `-le init+n-1` with n == 0 and init == 0 would be `-le 0`: one
        // trip. Harmless: the product bound only needs to be approximate.
        let inc = match self.rng.range(0, 3) {
            0 => format!("{var}=$(({var}+1))"),
            1 => format!("(({var}++))"),
            2 => format!("let {var}++"),
            _ => format!("{var}=$(({var} + 1))"),
        };
        let at_end = self.rng.chance(1, 2);
        let l = Loop {
            counter_at_end: at_end,
            var: var.clone(),
            numeric: true,
        };
        let body = self.with_loop(l, n + 1, |g| g.block(depth + 1));
        let body = if at_end {
            format!("{body}; {inc}")
        } else {
            format!("{inc}; {body}")
        };
        Some(format!("{var}={init}; {cmp} {bound}; do {body}; done"))
    }

    fn if_stmt(&mut self, depth: usize) -> String {
        let c = self.cond();
        let t = self.block(depth + 1);
        if self.rng.chance(1, 2) {
            let e = self.block(depth + 1);
            format!("if {c}; then {t}; else {e}; fi")
        } else {
            format!("if {c}; then {t}; fi")
        }
    }

    fn case_stmt(&mut self, depth: usize) -> String {
        let subject = match self.loops.last() {
            Some(l) if self.rng.chance(2, 3) => format!("\"${}\"", l.var),
            _ => "\"$k\"".to_string(),
        };
        let a = self.block(depth + 1);
        let b = self.block(depth + 1);
        let mut s = format!("case {subject} in a|1) {a};; b|2) {b};;");
        if self.rng.chance(1, 2) {
            let d = self.block(depth + 1);
            s.push_str(&format!(" *) {d};;"));
        }
        s.push_str(" esac");
        s
    }
}
