//! `kubectl-gitops` — changing a cluster by hand that git is supposed to own.
//!
//! In a repository reconciled by Flux or Argo CD, the manifests ARE the
//! cluster. `kubectl apply -f`, `kubectl patch`, `kubectl scale` from the
//! terminal make a change the reconciler either reverts on its next sync —
//! so the fix evaporates — or keeps, so the repository no longer describes
//! what is running and the next bootstrap silently loses it. The user's own
//! words, 2026-09-02: "we use gitops, not sure you should even kubectl apply
//! -f". Measured over forty-two sessions: 255 imperative `kubectl` mutations.
//!
//! ## What is not a fire
//!
//! Reads (`get`, `logs`, `describe`, `top`, `exec`) are not mutations. A
//! `--dry-run` mutates nothing. Deleting a pod, a job or a replica set is a
//! RUNTIME action on something a controller re-creates — a restart, not
//! drift — and `rollout restart` is the same thing spelled properly. Those
//! stay silent.
//!
//! ## `confirm` asks whether git owns this cluster
//!
//! The shape fires everywhere; the fact lives in the repository the command
//! runs from. `confirm` looks for a Flux or Argo resource anywhere in that
//! repository's tracked files and stays silent when there is none — a
//! kind cluster in a scratch directory is nobody's GitOps.

use crate::rules::{Confirmed, Context, Evidence, Finding, Rule, Stance, Trend};
use crate::shell::{Parsed, Simple};

pub const RULE: Rule = Rule {
    id: "kubectl-gitops",
    default_stance: Stance::Advise,
    evidence: Evidence {
        per_1000: 13.1,
        measured: "2026-09-05",
        trend: Trend::Flat(8),
    },
    examine,
    confirm: Some(confirm),
};

/// Verbs that write a resource the repository would otherwise describe.
const MUTATING: &[&str] = &[
    "apply", "create", "patch", "edit", "replace", "scale", "annotate", "label", "set", "delete",
];

/// Resources a controller re-creates: deleting one is a restart, not drift.
const RUNTIME_OWNED: &[&str] = &[
    "pod",
    "pods",
    "po",
    "job",
    "jobs",
    "replicaset",
    "replicasets",
    "rs",
    "event",
    "events",
    "ev",
];

/// kubectl's own options that sit BEFORE the verb and take a value.
const GLOBAL_VALUED: &[&str] = &[
    "-n",
    "--namespace",
    "--context",
    "--kubeconfig",
    "--cluster",
    "--user",
    "--server",
    "-s",
    "--token",
    "--as",
    "--as-group",
    "--request-timeout",
    "-v",
];

/// The verb and the first operand after it, with kubectl's global options
/// peeled off. `None` when the verb cannot be told.
fn verb_of(cmd: &Simple) -> Option<(&str, Option<&str>)> {
    let program = cmd.program()?;
    let start = cmd.words.iter().position(|w| w.text == program)? + 1;
    let words: Vec<&crate::shell::Word> = cmd.words.iter().skip(start).collect();
    let mut i = 0;
    let verb = loop {
        let w = words.get(i)?;
        let t = w.text.as_str();
        if w.quoted || !t.starts_with('-') {
            break t;
        }
        if GLOBAL_VALUED.contains(&t) {
            i += 2;
        } else {
            // `--namespace=x`, `-A`, `--all-namespaces`: one token.
            i += 1;
        }
    };
    let mut resource = None;
    let mut j = i + 1;
    while let Some(w) = words.get(j) {
        let t = w.text.as_str();
        if w.quoted || !t.starts_with('-') {
            resource = Some(t);
            break;
        }
        if GLOBAL_VALUED.contains(&t)
            || matches!(
                t,
                "-f" | "--filename"
                    | "-k"
                    | "-l"
                    | "--selector"
                    | "-p"
                    | "--patch"
                    | "--type"
                    | "--replicas"
            )
        {
            j += 2;
        } else {
            j += 1;
        }
    }
    Some((verb, resource))
}

fn detect(cmd: &Simple) -> bool {
    if cmd.program() != Some("kubectl") || cmd.is_dry_run() {
        return false;
    }
    let Some((verb, resource)) = verb_of(cmd) else {
        return false;
    };
    if !MUTATING.contains(&verb) {
        return false;
    }
    if verb == "delete" {
        // `kubectl delete pod x`: the controller brings it back.
        if let Some(r) = resource {
            let kind = r.split('/').next().unwrap_or(r);
            if RUNTIME_OWNED.contains(&kind) {
                return false;
            }
        }
    }
    true
}

fn examine(parsed: &Parsed) -> Option<Finding> {
    let cmd = parsed.clauses().iter().find(|c| detect(c))?;
    Some(Finding {
        reason: "this cluster is reconciled from git: a resource changed by hand is drift \
                 — the controller reverts it on its next sync and the fix evaporates, or \
                 keeps it and the repository no longer describes what is running."
            .to_string(),
        remedy: "Change the manifest in the repository and let the reconciler apply it \
                 (`flux reconcile kustomization <name> --with-source` to hurry it). Use \
                 `kubectl` here to read, or for what no manifest owns — restarting a pod \
                 is fine."
            .to_string(),
        span: cmd.at..cmd.end,
    })
}

/// Markers of a repository that a reconciler reads. One `git grep` over the
/// tracked YAML, bounded to the first hit.
const MARKERS: &[&str] = &[
    "kustomize.toolkit.fluxcd.io",
    "helm.toolkit.fluxcd.io",
    "source.toolkit.fluxcd.io",
    "argoproj.io/v1alpha1",
];

fn confirm(ctx: &Context, f: &Finding) -> Confirmed {
    let cwd = ctx.cwd_at(f.span.start);
    if !cwd.is_dir() {
        return Confirmed::No("the directory the command moves to does not exist");
    }
    let Some(top) = crate::git::stdout_in(&cwd, &["rev-parse", "--show-toplevel"]) else {
        return Confirmed::No("not inside a git repository");
    };
    let mut args: Vec<&str> = vec!["grep", "-l", "-I", "-m", "1"];
    for m in MARKERS {
        args.push("-e");
        args.push(m);
    }
    args.extend(["--", "*.yaml", "*.yml"]);
    if crate::git::succeeds_in(std::path::Path::new(&top), &args) {
        Confirmed::Yes
    } else {
        Confirmed::No("no Flux or Argo resource in this repository")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::lex;

    fn fires(command: &str) -> bool {
        examine(&lex(command)).is_some()
    }

    #[test]
    fn an_imperative_write_has_the_shape() {
        assert!(fires("kubectl apply -f manifest.yaml"));
        assert!(fires(
            "kubectl -n trade-agents delete deployment ibkr-ingester"
        ));
        assert!(fires("kubectl -n keda scale deploy runner --replicas=0"));
        assert!(fires(
            "kubectl patch kustomization apps -n flux-system --type merge -p '{\"spec\":{\"suspend\":true}}'"
        ));
        assert!(fires("cat manifest.yaml | kubectl apply -f -"));
        assert!(fires("kubectl --context=cloud create namespace stalwart"));
        assert!(fires("kubectl delete -f kubernetes/apps/x.yaml"));
    }

    #[test]
    fn reads_and_dry_runs_are_silent() {
        assert!(!fires(
            "kubectl -n trade-agents get pods --no-headers | grep ibkr"
        ));
        assert!(!fires("kubectl apply --dry-run=client -f x.yaml"));
        assert!(!fires("kubectl -n velero logs deploy/velero --since=30m"));
        assert!(!fires("kubectl describe kustomization apps -n flux-system"));
        assert!(!fires("kubectl exec -n stremio pod/x -- ls"));
    }

    #[test]
    fn a_restart_is_not_drift() {
        assert!(!fires(
            "kubectl delete pod x --wait=false; kubectl get pods | head"
        ));
        assert!(!fires("kubectl -n ci delete job runner-123"));
        assert!(!fires("kubectl delete pod/x -n y"));
        assert!(!fires("kubectl rollout restart deploy/x -n y"));
    }

    #[test]
    fn the_verb_is_found_behind_global_options() {
        let parsed = lex("kubectl -n ns --context c apply -f x.yaml");
        let cmd = &parsed.clauses()[0];
        assert_eq!(verb_of(cmd), Some(("apply", None)));
        let parsed = lex("kubectl delete -n ns deployment web");
        let cmd = &parsed.clauses()[0];
        assert_eq!(verb_of(cmd), Some(("delete", Some("deployment"))));
    }
}
