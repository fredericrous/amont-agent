//! The rule table, in the order every reader sees it: assembled here, at
//! the composition root (decisions ADR-0011 `systems.construction-in-main`),
//! because it holds the guard's rules and the work process's `push-preview`.
//! The guard's readers take it as a parameter, never name it.

use amont_agent_guard::rules::Rule;

pub const RULES: &[Rule] = &[
    crate::rules::pipe_to_tail::RULE,
    crate::rules::bare_stash_pop::RULE,
    crate::rules::cd_carries_over::RULE,
    crate::rules::gh_pr_merge_auto::RULE,
    crate::rules::publish_without_skill::RULE,
    crate::rules::forge_merge_by_hand::RULE,
    crate::rules::no_verify::RULE,
    crate::rules::git_add_broad::RULE,
    crate::rules::stale_base::RULE,
    crate::rules::push_preflight::RULE,
    crate::push_preview::RULE,
    crate::rules::plan_review_panel::RULE,
    crate::rules::plan_phases_open::RULE,
    crate::implementation_review_rule::RULE,
    crate::rules::foreground_poll::RULE,
    crate::rules::unbounded_background_push::RULE,
    crate::rules::gui_open_unbounded::RULE,
    crate::rules::sed_in_place::RULE,
    crate::rules::kubectl_gitops::RULE,
    crate::rules::tag_after_commit::RULE,
    crate::rules::release_tag_push::RULE,
    crate::rules::worktree_remove_force::RULE,
    crate::rules::amend_pushed::RULE,
    crate::rules::branch_force_delete::RULE,
    crate::rules::worktree_isolation::RULE,
    crate::rules::stdin_hang::RULE,
    crate::rules::glob_in_flag_value::RULE,
    crate::rules::glob_no_match::RULE,
    crate::rules::equals_separator::RULE,
    crate::rules::unsplit_expansion::RULE,
    crate::rules::path_operand_missing::RULE,
    crate::rules::stat_bsd_format::RULE,
    crate::rules::whole_file_dump::RULE,
    crate::rules::persisted_output_dump::RULE,
    crate::rules::file_reread::RULE,
    crate::rules::read_unbounded_large::RULE,
    crate::rules::code_read_not_lsp::RULE,
    crate::rules::symbol_grep_not_lsp::RULE,
    crate::rules::lint_suppression_added::RULE,
    crate::rules::agent_guidance_written::RULE,
    crate::rules::agent_guidance_shape::RULE,
    crate::rules::adr_lint_written::RULE,
    crate::rules::test_output_trimmed::RULE,
    crate::rules::workspace_root_write::RULE,
    crate::rules::heavy_build_unqueued::RULE,
];

/// The tests that read the whole table live here, where the whole table is:
/// the guard sees only its own rules.
#[cfg(test)]
mod tests {
    use super::RULES;
    use amont_agent_guard::corpus;

    /// ADR-0042: every part names the failure it prevents, in one sentence.
    #[test]
    fn every_rule_and_assertion_names_its_failure() {
        let parts = super::RULES.iter().map(|r| (r.id, r.prevents)).chain(
            amont_agent_guard::assertions::ASSERTIONS
                .iter()
                .map(|a| (a.id, a.prevents)),
        );
        for (id, prevents) in parts {
            let n = prevents.chars().count();
            assert!(n > 0, "`{id}` has an empty `prevents`");
            assert!(
                n < 140,
                "`{id}` prevents is {n} characters; keep it under 140"
            );
        }
    }

    /// The config key is derived from the id, and git subsections are
    /// case-sensitive. An id with a capital in it would produce a key nobody
    /// could set from the shell without knowing that.
    #[test]
    fn rule_ids_are_lowercase_kebab() {
        for r in RULES {
            assert!(
                r.id.bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-'),
                "rule id `{}` is not lowercase kebab-case",
                r.id
            );
            assert!(!r.id.starts_with('-') && !r.id.ends_with('-'), "{}", r.id);
        }
    }

    /// A rule id appearing twice would mean two rules sharing one config key,
    /// so setting the stance of one would silently move the other.
    #[test]
    fn rule_ids_are_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for r in RULES {
            assert!(seen.insert(r.id), "duplicate rule id `{}`", r.id);
        }
    }

    /// No rule ships above its own ceiling.
    #[test]
    fn no_rule_ships_above_its_ceiling() {
        for r in RULES {
            assert!(
                r.max_stance >= r.default_stance,
                "`{}` ships above its own ceiling",
                r.id
            );
        }
    }

    /// A rule without a compiled-in corpus would show "0 reviewed cases"
    /// from every installed copy — the exact failure this table exists to
    /// end — while passing the test suite, which reads the files.
    #[test]
    fn every_rule_has_an_embedded_corpus() {
        for rule in RULES {
            let text = corpus::embedded(rule.id)
                .unwrap_or_else(|| panic!("rule `{}` has no entry in EMBEDDED", rule.id));
            assert!(
                text.starts_with(corpus::HEADER),
                "embedded corpus for `{}` lacks the header",
                rule.id
            );
        }
    }

    /// The compiled-in copy is the file: same cases, same verdicts.
    #[test]
    fn the_embedded_corpus_is_the_file() {
        for rule in RULES {
            let from_file = corpus::read_at(&corpus::path_for(rule.id));
            let from_binary = corpus::parse(corpus::embedded(rule.id).unwrap());
            assert_eq!(from_file.len(), from_binary.len(), "{}", rule.id);
        }
    }

    /// `rules --json` writes every rate as a JSON number, and JSON has no
    /// NaN or infinity: `json::float_field` asserts it, this keeps it true.
    #[test]
    fn every_measured_rate_is_a_json_number() {
        for r in RULES {
            assert!(r.evidence.per_1000.is_finite(), "{}", r.id);
        }
        for a in amont_agent_guard::assertions::ASSERTIONS {
            assert!(a.evidence.per_1000.is_finite(), "{}", a.id);
        }
    }

    /// The table holds every rule, in its place: 45 on main before the
    /// workspace split (42 after three observe rules were retired), with `push-preview` between `push-preflight` and
    /// `plan-review-panel`, and `implementation-review` after
    /// `plan-phases-open`. A rule a crate stopped listing would vanish here.
    #[test]
    fn the_table_holds_every_rule_in_its_place() {
        let ids: Vec<&str> = RULES.iter().map(|r| r.id).collect();
        assert_eq!(ids.len(), 45, "{ids:?}");
        let at = |id: &str| {
            ids.iter()
                .position(|x| *x == id)
                .unwrap_or_else(|| panic!("{id} missing"))
        };
        assert_eq!(at("push-preview"), at("push-preflight") + 1);
        assert_eq!(at("plan-review-panel"), at("push-preview") + 1);
        assert_eq!(at("implementation-review"), at("plan-phases-open") + 1);
    }
}
