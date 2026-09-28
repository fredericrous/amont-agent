//! Stands in for `curl`, `wget` and `gh` while a generated script runs.
//!
//! Installed as symlinks named after each program; `argv[0]` says which one
//! it is. Every transfer the real program would make for its explicit
//! command-line URLs is one line `<program> <host>` appended to
//! `$ORACLE_LOG`. It prints nothing and exits 0.
//!
//! `$ORACLE_BUDGET`: once the log holds more lines than this, the stub kills
//! its whole process group (the harness starts each shell as a group
//! leader), which is how a script that never ends is stopped at a known
//! count instead of by a wall-clock timeout.

use std::io::Write;

fn host_of(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let host = &host[..host.find(':').unwrap_or(host.len())];
    Some(host.to_ascii_lowercase())
}

fn main() {
    let mut args = std::env::args();
    let argv0 = args.next().unwrap_or_default();
    let program = argv0.rsplit('/').next().unwrap_or(&argv0).to_string();
    let rest: Vec<String> = args.collect();

    let hosts: Vec<String> = match program.as_str() {
        "gh" => {
            if rest.first().map(String::as_str) == Some("api") {
                vec!["api.github.com".into()]
            } else {
                Vec::new()
            }
        }
        _ => {
            let found: Vec<String> = rest.iter().filter_map(|a| host_of(a)).collect();
            if found.is_empty() {
                vec!["?".into()]
            } else {
                found
            }
        }
    };

    let Ok(log) = std::env::var("ORACLE_LOG") else {
        return;
    };
    {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
            .expect("open ORACLE_LOG");
        let mut buf = String::new();
        for h in &hosts {
            buf.push_str(&format!("{program} {h}\n"));
        }
        // One write per invocation: O_APPEND keeps concurrent stubs (a
        // pipeline of two clients) from interleaving inside a line.
        f.write_all(buf.as_bytes()).expect("append ORACLE_LOG");
    }

    if let Some(budget) = std::env::var("ORACLE_BUDGET")
        .ok()
        .and_then(|b| b.parse::<usize>().ok())
    {
        let lines = std::fs::read_to_string(&log)
            .map(|s| s.lines().count())
            .unwrap_or(0);
        if lines > budget {
            // `kill 0`: every process in our group — the shell under test,
            // any subshells, this stub.
            let _ = std::process::Command::new("kill")
                .args(["-KILL", "0"])
                .status();
            std::process::exit(137);
        }
    }
}
