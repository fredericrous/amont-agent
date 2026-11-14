//! The `clear-relay` Claude Code plugin, embedded and placed by `install`.
//!
//! The files live under `plugin/clear-relay/` and are compiled in with
//! `include_str!`, so the binary carries what it places and a release cannot
//! drift from the plugin it was tested with. `tests/plugin_install.rs` fails
//! when a file is added there and not listed here.

use std::path::PathBuf;

use amont_agent_guard::settings::{self, PluginPlacement};

pub const MARKETPLACE: &str = "amont-agent";
pub const PLUGIN: &str = "clear-relay";

const MARKETPLACE_JSON: &str = r#"{
  "name": "amont-agent",
  "owner": {
    "name": "amont-agent"
  },
  "plugins": [
    {
      "description": "clear-relay: clears the session and carries on by itself when an answer ends with the handoff line",
      "name": "clear-relay",
      "source": "./plugins/clear-relay"
    }
  ]
}
"#;

/// Every file of `plugin/clear-relay/`, by path relative to it.
const PLUGIN_FILES: &[(&str, &str)] = &[
    (
        ".claude-plugin/plugin.json",
        include_str!("../plugin/clear-relay/.claude-plugin/plugin.json"),
    ),
    (
        "hooks/hooks.json",
        include_str!("../plugin/clear-relay/hooks/hooks.json"),
    ),
    (
        "hooks/register.ts",
        include_str!("../plugin/clear-relay/hooks/register.ts"),
    ),
    (
        "hooks/register.test.ts",
        include_str!("../plugin/clear-relay/hooks/register.test.ts"),
    ),
];

/// Where the plugin goes and what it holds, or `None` when no state directory
/// can be resolved (then only the hooks are installed).
pub fn placement() -> Option<PluginPlacement> {
    let state_root = settings::state_root()?;
    let state_dir = state_root.join("amont-agent");
    let mut files: Vec<(PathBuf, String)> = vec![(
        PathBuf::from(".claude-plugin/marketplace.json"),
        MARKETPLACE_JSON.to_string(),
    )];
    files.extend(PLUGIN_FILES.iter().map(|(rel, body)| {
        (
            PathBuf::from("plugins").join(PLUGIN).join(rel),
            (*body).to_string(),
        )
    }));
    Some(PluginPlacement {
        marketplace_dir: state_dir.join("claude-marketplace"),
        marketplace: MARKETPLACE,
        plugin: PLUGIN,
        files,
        state_dir,
    })
}
