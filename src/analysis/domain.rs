//! How many times something executes: an interval with an honest top.
//!
//! `lower` is what the analysis ESTABLISHED under its stated assumptions;
//! `upper` is what MAY happen. The distinction is the product: "at least 1,
//! at most 400" and "exactly 400" are different findings, and a policy that
//! cannot tell them apart would state potential repetition as fact.
//!
//! The upper bound has four shapes, and each means something different:
//!
//! - `Finite(n)`: at most `n`.
//! - `Saturated`: finite, but more than a `u64` can hold. Overflow is NOT
//!   "unbounded": a loop nested in a loop has a bound, just a huge one.
//! - `Uncapped(why)`: the analysis has positive evidence that nothing bounds
//!   it — a loop that follows `next` links until they run out, `--paginate`.
//! - `Unknown`: the analysis could not tell. Never promoted to `Uncapped`,
//!   never demoted to a number.
//!
//! The algebra is written out as tables in the tests below, one assertion per
//! cell, because every rule built on this inherits its mistakes.

use std::fmt;

/// Why nothing bounds a count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Why {
    /// A loop that reassigns its condition from a value its own transfers
    /// produced — pagination by `Link`/`next`.
    NextLinkFollow,
    /// A client that follows pagination itself (`gh api --paginate`).
    Paginate,
    /// A client that recurses on its own (`wget -r`).
    Recursive,
    /// `while true` with no reachable way out.
    Infinite,
}

impl Why {
    pub fn describe(self) -> &'static str {
        match self {
            Why::NextLinkFollow => "follows next-page links until they run out",
            Why::Paginate => "follows the API's pagination until it runs out",
            Why::Recursive => "recurses through links",
            Why::Infinite => "loops forever",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Upper {
    Finite(u64),
    Saturated,
    Uncapped(Why),
    Unknown,
}

/// An execution count: at least `lower`, at most `upper`.
///
/// Invariant: when `upper` is `Finite(u)`, `lower <= u`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Count {
    pub lower: u64,
    pub upper: Upper,
}

impl Count {
    pub const ZERO: Count = Count {
        lower: 0,
        upper: Upper::Finite(0),
    };
    pub const ONE: Count = Count {
        lower: 1,
        upper: Upper::Finite(1),
    };
    /// Runs at most once, possibly not at all.
    pub const MAYBE: Count = Count {
        lower: 0,
        upper: Upper::Finite(1),
    };
    pub const UNKNOWN: Count = Count {
        lower: 0,
        upper: Upper::Unknown,
    };

    pub fn exactly(n: u64) -> Count {
        Count {
            lower: n,
            upper: Upper::Finite(n),
        }
    }

    pub fn between(lower: u64, upper: u64) -> Count {
        debug_assert!(lower <= upper);
        Count {
            lower: lower.min(upper),
            upper: Upper::Finite(upper),
        }
    }

    pub fn at_most(upper: u64) -> Count {
        Count {
            lower: 0,
            upper: Upper::Finite(upper),
        }
    }

    pub fn uncapped(lower: u64, why: Why) -> Count {
        Count {
            lower,
            upper: Upper::Uncapped(why),
        }
    }

    /// Proven never to happen.
    pub fn is_zero(self) -> bool {
        self.upper == Upper::Finite(0)
    }

    /// Sequential nesting: `self` executions, each doing `other`.
    pub fn mul(self, other: Count) -> Count {
        // A proven zero annihilates everything, including Unknown and
        // Uncapped: a loop that provably never runs its body makes no
        // requests, however many that body would have made.
        if self.is_zero() || other.is_zero() {
            return Count::ZERO;
        }
        let upper = match (self.upper, other.upper) {
            (Upper::Unknown, _) | (_, Upper::Unknown) => Upper::Unknown,
            (Upper::Uncapped(w), _) | (_, Upper::Uncapped(w)) => Upper::Uncapped(w),
            (Upper::Saturated, _) | (_, Upper::Saturated) => Upper::Saturated,
            (Upper::Finite(a), Upper::Finite(b)) => match a.checked_mul(b) {
                Some(n) => Upper::Finite(n),
                None => Upper::Saturated,
            },
        };
        Count {
            lower: self.lower.saturating_mul(other.lower),
            upper,
        }
    }

    /// Sequence: `self` and then `other`.
    pub fn add(self, other: Count) -> Count {
        let upper = match (self.upper, other.upper) {
            (Upper::Unknown, _) | (_, Upper::Unknown) => Upper::Unknown,
            (Upper::Uncapped(w), _) | (_, Upper::Uncapped(w)) => Upper::Uncapped(w),
            (Upper::Saturated, _) | (_, Upper::Saturated) => Upper::Saturated,
            (Upper::Finite(a), Upper::Finite(b)) => match a.checked_add(b) {
                Some(n) => Upper::Finite(n),
                None => Upper::Saturated,
            },
        };
        Count {
            lower: self.lower.saturating_add(other.lower),
            upper,
        }
    }

    /// Alternatives: one of `self` or `other` happens, not both.
    pub fn join(self, other: Count) -> Count {
        let upper = match (self.upper, other.upper) {
            (Upper::Unknown, _) | (_, Upper::Unknown) => Upper::Unknown,
            (Upper::Uncapped(w), _) | (_, Upper::Uncapped(w)) => Upper::Uncapped(w),
            (Upper::Saturated, _) | (_, Upper::Saturated) => Upper::Saturated,
            (Upper::Finite(a), Upper::Finite(b)) => Upper::Finite(a.max(b)),
        };
        Count {
            lower: self.lower.min(other.lower),
            upper,
        }
    }

    /// `self` minus one execution, never below zero — the iterations after
    /// the first, when the first is analysed on its own.
    pub fn minus_one(self) -> Count {
        let upper = match self.upper {
            Upper::Finite(n) => Upper::Finite(n.saturating_sub(1)),
            other => other,
        };
        Count {
            lower: self.lower.saturating_sub(1),
            upper,
        }
    }

    /// Whether `upper` may exceed `n`: `Finite(>n)`, `Saturated` or
    /// `Uncapped`. `Unknown` answers no — it is not evidence of anything.
    pub fn may_exceed(self, n: u64) -> bool {
        match self.upper {
            Upper::Finite(u) => u > n,
            Upper::Saturated | Upper::Uncapped(_) => true,
            Upper::Unknown => false,
        }
    }
}

impl fmt::Display for Count {
    /// `400`, `1–400`, `at least 1 (no established cap: …)`, `unknown`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.upper {
            Upper::Finite(u) if u == self.lower => write!(f, "{u}"),
            Upper::Finite(u) if self.lower == 0 => write!(f, "up to {u}"),
            Upper::Finite(u) => write!(f, "{}–{u}", self.lower),
            Upper::Saturated => write!(f, "more than {}", u64::MAX),
            Upper::Uncapped(w) => write!(f, "unbounded ({})", w.describe()),
            Upper::Unknown => write!(f, "an unknown number of"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: Why = Why::Paginate;
    fn fin(l: u64, u: u64) -> Count {
        Count::between(l, u)
    }
    fn unc(l: u64) -> Count {
        Count::uncapped(l, W)
    }
    fn sat(l: u64) -> Count {
        Count {
            lower: l,
            upper: Upper::Saturated,
        }
    }
    fn unk(l: u64) -> Count {
        Count {
            lower: l,
            upper: Upper::Unknown,
        }
    }

    // ── multiply: one assertion per cell ───────────────────────────────────

    #[test]
    fn mul_zero_annihilates_every_shape() {
        for x in [fin(0, 5), fin(3, 3), unc(1), sat(2), unk(0)] {
            assert_eq!(Count::ZERO.mul(x), Count::ZERO, "0 × {x:?}");
            assert_eq!(x.mul(Count::ZERO), Count::ZERO, "{x:?} × 0");
        }
    }

    #[test]
    fn mul_finite_by_finite_multiplies_both_ends() {
        assert_eq!(fin(2, 3).mul(fin(4, 5)), fin(8, 15));
        assert_eq!(fin(0, 3).mul(fin(1, 5)), fin(0, 15));
    }

    #[test]
    fn mul_overflow_is_saturated_not_uncapped() {
        let big = fin(u64::MAX, u64::MAX);
        assert_eq!(big.mul(fin(2, 2)).upper, Upper::Saturated);
        assert_eq!(big.mul(fin(2, 2)).lower, u64::MAX, "lower saturates");
    }

    #[test]
    fn mul_saturated_by_finite_or_saturated_stays_saturated() {
        assert_eq!(sat(1).mul(fin(1, 2)).upper, Upper::Saturated);
        assert_eq!(fin(1, 2).mul(sat(1)).upper, Upper::Saturated);
        assert_eq!(sat(1).mul(sat(1)).upper, Upper::Saturated);
    }

    #[test]
    fn mul_uncapped_dominates_finite_and_saturated() {
        assert_eq!(unc(1).mul(fin(1, 3)).upper, Upper::Uncapped(W));
        assert_eq!(fin(1, 3).mul(unc(1)).upper, Upper::Uncapped(W));
        assert_eq!(unc(1).mul(sat(1)).upper, Upper::Uncapped(W));
        assert_eq!(unc(1).mul(unc(1)).upper, Upper::Uncapped(W));
    }

    /// Unknown × Uncapped is Unknown: the unknown side may be zero, so the
    /// product is not established to be uncapped.
    #[test]
    fn mul_unknown_absorbs_everything_but_zero() {
        for x in [fin(1, 3), sat(1), unc(1), unk(0)] {
            assert_eq!(unk(0).mul(x).upper, Upper::Unknown, "unknown × {x:?}");
            assert_eq!(x.mul(unk(0)).upper, Upper::Unknown, "{x:?} × unknown");
        }
    }

    #[test]
    fn mul_lower_bounds_multiply_and_saturate() {
        assert_eq!(unc(3).mul(fin(2, 2)).lower, 6);
        assert_eq!(unk(2).mul(fin(3, 4)).lower, 6);
        assert_eq!(fin(u64::MAX, u64::MAX).mul(unc(2)).lower, u64::MAX);
    }

    // ── add ───────────────────────────────────────────────────────────────

    #[test]
    fn add_finite_adds_both_ends() {
        assert_eq!(fin(1, 2).add(fin(3, 4)), fin(4, 6));
    }

    #[test]
    fn add_does_not_annihilate() {
        assert_eq!(Count::ZERO.add(fin(1, 2)), fin(1, 2));
        assert_eq!(Count::ZERO.add(unc(0)).upper, Upper::Uncapped(W));
        assert_eq!(Count::ZERO.add(unk(0)).upper, Upper::Unknown);
    }

    #[test]
    fn add_overflow_is_saturated() {
        assert_eq!(fin(0, u64::MAX).add(fin(0, 1)).upper, Upper::Saturated);
        assert_eq!(sat(0).add(fin(0, 1)).upper, Upper::Saturated);
    }

    #[test]
    fn add_precedence_unknown_then_uncapped_then_saturated() {
        assert_eq!(unk(0).add(unc(0)).upper, Upper::Unknown);
        assert_eq!(unc(0).add(sat(0)).upper, Upper::Uncapped(W));
        assert_eq!(sat(0).add(fin(0, 1)).upper, Upper::Saturated);
    }

    #[test]
    fn add_keeps_the_established_part_of_an_unknown_sum() {
        assert_eq!(unk(0).add(fin(100, 100)).lower, 100);
    }

    // ── join (alternatives) ───────────────────────────────────────────────

    #[test]
    fn join_is_min_lower_max_upper() {
        assert_eq!(fin(1, 1).join(fin(1, 1)), fin(1, 1));
        assert_eq!(fin(1, 1).join(Count::ZERO), fin(0, 1));
        assert_eq!(fin(2, 5).join(fin(3, 9)), fin(2, 9));
    }

    #[test]
    fn join_precedence_unknown_then_uncapped_then_saturated_then_finite() {
        assert_eq!(unk(0).join(unc(0)).upper, Upper::Unknown);
        assert_eq!(unc(0).join(sat(0)).upper, Upper::Uncapped(W));
        assert_eq!(sat(0).join(fin(0, 9)).upper, Upper::Saturated);
    }

    // ── helpers ───────────────────────────────────────────────────────────

    #[test]
    fn minus_one_peels_the_first_iteration() {
        assert_eq!(fin(400, 400).minus_one(), fin(399, 399));
        assert_eq!(fin(1, 400).minus_one(), fin(0, 399));
        assert_eq!(Count::ZERO.minus_one(), Count::ZERO);
        assert_eq!(unc(1).minus_one(), unc(0));
    }

    #[test]
    fn unknown_is_never_evidence_of_excess() {
        assert!(!unk(0).may_exceed(50));
        assert!(unc(0).may_exceed(50));
        assert!(sat(0).may_exceed(50));
        assert!(fin(0, 51).may_exceed(50));
        assert!(!fin(0, 50).may_exceed(50));
    }

    /// `lower <= upper` holds through every operation on finite inputs.
    #[test]
    fn lower_never_exceeds_a_finite_upper() {
        let xs = [
            Count::ZERO,
            Count::ONE,
            Count::MAYBE,
            fin(2, 7),
            fin(0, 3),
            fin(5, 5),
        ];
        for a in xs {
            for b in xs {
                for c in [a.mul(b), a.add(b), a.join(b), a.minus_one()] {
                    if let Upper::Finite(u) = c.upper {
                        assert!(c.lower <= u, "{a:?} {b:?} → {c:?}");
                    }
                }
            }
        }
    }

    /// Raising a factor never lowers a product's upper bound.
    #[test]
    fn mul_is_monotone_in_the_upper_bound() {
        for n in 0..20u64 {
            let small = fin(0, n).mul(fin(1, 3));
            let large = fin(0, n + 1).mul(fin(1, 3));
            match (small.upper, large.upper) {
                (Upper::Finite(a), Upper::Finite(b)) => assert!(a <= b),
                other => panic!("unexpected {other:?}"),
            }
        }
    }
}
