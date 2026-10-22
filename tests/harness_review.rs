//! `harness-review --check` through the binary (ADR-0042,
//! `harness.revisit-on-a-new-model`): it exits 1 naming a model no review has
//! covered, and exits 0, silent, once a review of that model exists.

use std::path::PathBuf;
use std::process::Command;

struct Home(PathBuf);

impl Home {
    fn new(name: &str) -> Home {
        let dir =
            std::env::temp_dir().join(format!("amont-agent-hr-it-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let proj = dir.join("projects").join("p");
        std::fs::create_dir_all(&proj).expect("projects dir");
        Home(dir)
    }
    fn session(&self, file: &str, model: &str, day: &str) {
        let line = format!(
            "{{\"timestamp\":\"{day}T10:00:00Z\",\"sessionId\":\"s-{file}\",\"cwd\":\"/w\",\
             \"message\":{{\"model\":\"{model}\",\"content\":[{{\"type\":\"tool_use\",\
             \"id\":\"t-{file}\",\"name\":\"Bash\",\"input\":{{\"command\":\"ls\"}}}}]}}}}\n"
        );
        std::fs::write(
            self.0
                .join("projects")
                .join("p")
                .join(format!("{file}.jsonl")),
            line,
        )
        .expect("a transcript");
    }
    /// Where every run writes its review: a temporary `--out`, never the
    /// real review directory.
    fn out(&self) -> PathBuf {
        self.0.join("reviews-out")
    }
    fn run(&self, args: &[&str]) -> (i32, String) {
        let (code, out, _) = self.run_err(args);
        (code, out)
    }
    fn run_err(&self, args: &[&str]) -> (i32, String, String) {
        let out = Command::new(env!("CARGO_BIN_EXE_amont-agent"))
            .arg("harness-review")
            .args(args)
            .arg("--out")
            .arg(self.out())
            .env("CLAUDE_CONFIG_DIR", &self.0)
            .env_remove("AMONT_AGENT_OFF")
            .output()
            .expect("the binary runs");
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn check_exits_1_on_an_unseen_model_and_0_after_a_review_of_it() {
    let h = Home::new("check");
    h.session("a", "claude-one", "2026-10-01");
    h.session("a2", "claude-one", "2026-10-01");

    let (code, said) = h.run(&["--check"]);
    assert_eq!(code, 1, "no review on record: {said}");
    assert!(
        said.contains("claude-one") && said.lines().count() == 1,
        "{said}"
    );

    let (code, json) = h.run(&["--json"]);
    assert_eq!(code, 0);
    assert!(json.contains("\"newest_model\":\"claude-one\""), "{json}");
    assert!(
        h.out().read_dir().expect("review dir").count() == 1,
        "one review file written"
    );

    let (code, said) = h.run(&["--check"]);
    assert_eq!(
        (code, said.as_str()),
        (0, ""),
        "reviewed model prints nothing"
    );

    // A newer model with fewer calls (1 against 2) is not the default.
    h.session("b", "claude-two", "2026-10-09");
    let (code, said) = h.run(&["--check"]);
    assert_eq!((code, said.as_str()), (0, ""), "{said}");

    // One with more calls (3 against 2) is.
    h.session("c", "claude-two", "2026-10-09");
    h.session("d", "claude-two", "2026-10-09");
    let (code, said) = h.run(&["--check"]);
    assert_eq!(code, 1);
    assert!(said.contains("claude-two"), "{said}");
}

#[test]
fn the_json_has_one_row_per_rule_and_assertion() {
    let h = Home::new("rows");
    h.session("a", "claude-one", "2026-10-01");
    let (code, json) = h.run(&["--json"]);
    assert_eq!(code, 0);
    let out = Command::new(env!("CARGO_BIN_EXE_amont-agent"))
        .args(["rules", "--json"])
        .env("CLAUDE_CONFIG_DIR", &h.0)
        .output()
        .expect("the binary runs");
    assert_eq!(out.status.code(), Some(0), "rules --json exits 0");
    let rules = String::from_utf8_lossy(&out.stdout).into_owned();
    let rule_count = rules.matches("\"id\":").count();
    let row_count = json.matches("\"prevents\":").count();
    assert!(
        row_count == rule_count && rule_count > 0,
        "{row_count} rows, {rule_count} rules"
    );
}

#[test]
fn check_names_a_corrupt_review_file_and_exits_1() {
    let h = Home::new("corrupt");
    h.session("a", "claude-one", "2026-10-01");
    let dir = h.out();
    std::fs::create_dir_all(&dir).expect("review dir");
    std::fs::write(dir.join("2026-10-20.json"), "{not json").expect("corrupt file");
    let (code, said) = h.run(&["--check"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("2026-10-20.json"), "{said}");
    assert!(!said.contains("no review on record"), "{said}");
}

#[test]
fn help_prints_usage_and_an_operand_is_refused() {
    let h = Home::new("help");
    for flag in ["--help", "-h"] {
        let (code, said) = h.run(&[flag]);
        assert_eq!(code, 0);
        assert!(said.contains("usage:"), "{said}");
    }
    let (code, _, err) = h.run_err(&["bogus"]);
    assert_eq!(code, 2, "{err}");
}

#[test]
fn model_names_the_reviewed_model_and_the_output_says_which() {
    let h = Home::new("named");
    h.session("a", "claude-one", "2026-10-01");
    h.session("b", "claude-two", "2026-10-09");
    h.session("c", "claude-two", "2026-10-09");

    let (code, said) = h.run(&[]);
    assert_eq!(code, 0);
    assert!(
        said.contains("model: claude-two (default: the model with the most Bash calls)"),
        "{said}"
    );

    let (code, json) = h.run(&["--model", "claude-one", "--json"]);
    assert_eq!(code, 0);
    assert!(json.contains("\"model\":\"claude-one\""), "{json}");
    assert!(json.contains("\"newest_model\":\"claude-two\""), "{json}");
}

#[test]
fn check_reads_newest_model_from_a_review_file_without_model() {
    let h = Home::new("legacy-review");
    h.session("a", "claude-one", "2026-10-01");
    h.session("a2", "claude-one", "2026-10-01");
    h.session("b", "claude-two", "2026-10-09");
    // A 2.47.0 review file: `newest_model` only. claude-one is the most-called.
    let dir = h.out();
    std::fs::create_dir_all(&dir).expect("review dir");
    std::fs::write(
        dir.join("2026-10-20.json"),
        "{\"date\":\"2026-10-20\",\"newest_model\":\"claude-one\",\"rows\":[]}\n",
    )
    .expect("legacy review");
    let (code, said) = h.run(&["--check"]);
    assert_eq!((code, said.as_str()), (0, ""), "{said}");
}
