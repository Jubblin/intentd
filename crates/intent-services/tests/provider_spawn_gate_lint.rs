//! Source lint: every file in `crates/intent-services/src/` that starts a
//! process is classified, so a new provider spawn site cannot bypass the
//! orchestrator gate unnoticed (monorepo `docs/protocol/methods/nodes.md`,
//! "Orchestrator mode" → "One gate").
//!
//! An orchestrator never starts a provider process. Every site that does
//! asks `Services::may_spawn_provider_locally()`, at the site or at the
//! one entry point that reaches it. [`SITES`] is the golden list: each
//! non-test file that calls `Command::new(` is either a provider site,
//! with the gate that covers it, or another process (git, a user's shell,
//! an MCP server …) with why it is not a provider run. The lint fails on a
//! file that spawns and is not listed, on a listed file that no longer
//! spawns (stale), and on a provider row whose gate does not exist.
//! Test code is exempt: whole test files and `#[cfg(test)]` items as
//! `source_lint::test_code_files` classifies them, plus `test_support`.
//! Limit: a spawn written through an alias or a helper in another crate is
//! not seen.

use intentd_test_support::source_lint::{test_code_files, tokenize, workspace_root, TestCodeText};

const SRC_PREFIX: &str = "crates/intent-services/src/";

/// What a listed file's spawns are.
enum Spawn {
    /// A provider process (adapter, CLI probe, model or sign-in probe),
    /// gated by the named function, which must call
    /// `may_spawn_provider_locally` or sit behind one that does.
    Provider(&'static str),
    /// Not a provider run; the reason.
    Other(&'static str),
}

/// The golden list, by path under `crates/intent-services/src/`.
const SITES: &[(&str, Spawn)] = &[
    ("acp_adapter.rs", Spawn::Provider("complete_once_attempt / open_scratch_adapter / models_list_op")),
    ("agent_ops.rs", Spawn::Provider("models_list_op")),
    ("agy_cli.rs", Spawn::Provider("host.providerDiscovery / executor_provider_capabilities")),
    ("auggie_cli.rs", Spawn::Provider("host.providerDiscovery / executor_provider_capabilities")),
    ("bundled_cli.rs", Spawn::Provider("prepare_provider_adapters")),
    ("clone_ops.rs", Spawn::Other("git clone of a repository")),
    ("codex_diagnostics/catalog.rs", Spawn::Provider("models_list_op")),
    ("codex_diagnostics/mod.rs", Spawn::Provider("models_list_op")),
    ("cursor_cli.rs", Spawn::Provider("host.providerDiscovery / executor_provider_capabilities")),
    ("enhance_ops.rs", Spawn::Provider("agent_enhance_prompt_op / machine_one_shot")),
    ("host_exec.rs", Spawn::Other("the user's own command (host.exec)")),
    ("installed_cli.rs", Spawn::Provider("host.providerDiscovery / executor_provider_capabilities")),
    ("lib.rs", Spawn::Other("git reads of a workspace")),
    ("mcp_servers.rs", Spawn::Other("a user's stdio MCP server, not a provider run")),
    ("npx_cli.rs", Spawn::Provider("host.providerDiscovery / executor_provider_capabilities")),
    ("pi_cli.rs", Spawn::Provider("host.providerDiscovery / executor_provider_capabilities")),
    ("plugins/ops.rs", Spawn::Other("git fetch of an agent plugin")),
    ("provider_auth.rs", Spawn::Provider("host.providerAuthStatus / provider_profiles_auth_status_impl")),
    ("provider_cli.rs", Spawn::Provider("host.providerDiscovery")),
    ("provider_models/mod.rs", Spawn::Provider("models_list_op")),
    ("provider_version.rs", Spawn::Provider("host.providerDiscovery / executor_provider_capabilities")),
    ("repository_context_reader.rs", Spawn::Other("git reads of a repository")),
    ("rtk.rs", Spawn::Other("the rtk output compressor")),
    ("unsloth_server.rs", Spawn::Provider("create_agent (a local Unsloth agent)")),
];

/// The line of every non-test `Command::new(` in `src`.
fn spawns_in(src: &str, whole: bool) -> Vec<usize> {
    if whole {
        return Vec::new();
    }
    let text = TestCodeText::new(src, false);
    let tokens = tokenize(&text.chars);
    tokens
        .windows(4)
        .filter_map(|window| {
            let [ty, sep, new, open] = window else {
                return None;
            };
            (ty.is("Command")
                && sep.is("::")
                && new.is("new")
                && open.is("(")
                && !text.in_test(ty.start))
            .then_some(ty.line)
        })
        .collect()
}

fn is_test_support(inner: &str) -> bool {
    inner == "test_support.rs" || inner.starts_with("test_support/")
}

#[test]
fn every_process_spawn_site_is_classified() {
    let root = workspace_root();
    let mut found: Vec<(String, Vec<usize>)> = Vec::new();
    for file in test_code_files(&root) {
        let Some(inner) = file.rel.strip_prefix(SRC_PREFIX) else {
            continue;
        };
        if is_test_support(inner) {
            continue;
        }
        let lines = spawns_in(&file.src, file.whole);
        if !lines.is_empty() {
            found.push((inner.to_owned(), lines));
        }
    }
    let mut problems = Vec::new();
    for (inner, lines) in &found {
        if !SITES.iter().any(|(listed, _)| listed == inner) {
            problems.push(format!(
                "{SRC_PREFIX}{inner}:{} starts a process and is not in SITES: \
                 classify it (a provider spawn must pass may_spawn_provider_locally)",
                lines[0]
            ));
        }
    }
    for (listed, spawn) in SITES {
        if !found.iter().any(|(inner, _)| inner == listed) {
            problems.push(format!(
                "{SRC_PREFIX}{listed} is in SITES but starts no process: remove it"
            ));
        }
        let (Spawn::Provider(why) | Spawn::Other(why)) = spawn;
        assert!(!why.trim().is_empty(), "{listed}: an empty classification");
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// The gate the provider rows rely on exists and is checked where the
/// orchestrator turns provider work away.
#[test]
fn the_gate_is_called_at_the_entry_points() {
    let root = workspace_root();
    let read = |inner: &str| {
        std::fs::read_to_string(root.join(SRC_PREFIX).join(inner))
            .unwrap_or_else(|e| panic!("read {inner}: {e}"))
    };
    for inner in [
        "agent_manager.rs",
        "agent_manager/scratch_adapter.rs",
        "agent_ops.rs",
        "complete_ops.rs",
        "complete_ops/remote.rs",
        "enhance_ops.rs",
        "executor_capabilities.rs",
        "provider_profiles.rs",
        "lib.rs",
    ] {
        assert!(
            read(inner).contains("may_spawn_provider_locally()"),
            "{SRC_PREFIX}{inner} no longer asks may_spawn_provider_locally()"
        );
    }
}

#[test]
fn detector_sees_spawns_outside_test_code_only() {
    let src = "fn a() {\n    let c = Command::new(\"git\");\n}\n#[cfg(test)]\nmod tests {\n    fn b() { Command::new(\"x\"); }\n}\n";
    assert_eq!(spawns_in(src, false), vec![2]);
    assert!(spawns_in(src, true).is_empty());
    let qualified = "let c = tokio::process::Command::new(bin);\n// Command::new(x)\n";
    assert_eq!(spawns_in(qualified, false), vec![1]);
}
