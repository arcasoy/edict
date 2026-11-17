//! End-to-end tests: run the binary against fixture crates.

use std::path::PathBuf;
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn run(args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_cargo-edict"))
        .args(args)
        .output()
        .expect("binary runs");
    let text =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    (out.status.code().unwrap_or(-1), text)
}

fn json(name: &str) -> serde_json::Value {
    let path = fixture(name);
    let (_, text) = run(&[
        "edict",
        "--path",
        path.to_str().unwrap(),
        "--format",
        "json",
    ]);
    serde_json::from_str(&text).expect("valid json")
}

fn rules(v: &serde_json::Value) -> Vec<String> {
    v[0]["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["rule"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn clean_crate_passes() {
    let path = fixture("clean");
    let (code, text) = run(&["--path", path.to_str().unwrap()]);
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("0 errors"), "{text}");
}

#[test]
fn every_law_fires_on_violations() {
    let path = fixture("violations");
    let (code, text) = run(&["edict", "--path", path.to_str().unwrap()]);
    assert_eq!(code, 1, "{text}");
    for rule in [
        "cycles",
        "layers",
        "facade",
        "purity",
        "global-state",
        "limits",
    ] {
        assert!(
            text.contains(&format!("[{rule}]")),
            "missing {rule}:\n{text}"
        );
    }
}

#[test]
fn cycle_is_reported_with_both_hops() {
    let v = json("violations");
    let cycle = v[0]["violations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["rule"] == "cycles")
        .unwrap();
    assert_eq!(
        cycle["message"],
        "module cycle: billing::invoice -> billing::tax -> billing::invoice"
    );
    assert_eq!(cycle["notes"].as_array().unwrap().len(), 2);
}

#[test]
fn facade_sees_paths_inside_macros() {
    let v = json("violations");
    let lines: Vec<u64> = v[0]["violations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|x| {
            x["rule"] == "facade" && x["message"].as_str().unwrap().contains("reaches into")
        })
        .flat_map(|x| x["notes"].as_array().unwrap().clone())
        .map(|n| n["loc"]["line"].as_u64().unwrap())
        .collect();
    assert_eq!(
        lines,
        vec![4, 5],
        "format!() argument on line 5 should be caught"
    );
}

#[test]
fn global_state_respects_allowlist() {
    let v = json("violations");
    let msgs: Vec<String> = v[0]["violations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|x| x["rule"] == "global-state")
        .map(|x| x["message"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(msgs.len(), 4, "{msgs:?}");
    assert!(
        msgs.iter().all(|m| !m.contains("RUNTIME")),
        "infra::runtime is allowed"
    );
}

#[test]
fn limits_are_warnings_by_default() {
    let v = json("violations");
    let limit = v[0]["violations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["rule"] == "limits")
        .unwrap();
    assert_eq!(limit["severity"], "warning");
    assert!(rules(&v).contains(&"purity".to_string()));
}

#[test]
fn workspace_members_are_checked() {
    let path = fixture("workspace");
    let (code, text) = run(&["edict", "--path", path.to_str().unwrap()]);
    assert_eq!(code, 1, "{text}");
    assert!(text.contains("`ws_a`: 1 error"), "{text}");
    assert!(text.contains("`ws_b`: 0 errors"), "{text}");
}

#[test]
fn edict_obeys_its_own_laws() {
    let (code, text) = run(&["edict", "--path", env!("CARGO_MANIFEST_DIR")]);
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("0 warnings"), "{text}");
}

#[test]
fn glob_imports_resolve_through_barrels() {
    let v = json("gaps");
    let cycle = v[0]["violations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["rule"] == "cycles")
        .expect("glob cycle found");
    assert_eq!(cycle["message"], "module cycle: c -> d::inner -> c");
}

#[test]
fn glob_guesses_ignore_lowercase_locals() {
    let v = json("gaps");
    let notes: Vec<String> = v[0]["violations"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|x| x["notes"].as_array().unwrap().clone())
        .map(|n| n["message"].as_str().unwrap().to_string())
        .collect();
    assert!(notes.iter().all(|n| !n.contains("shadow")), "{notes:?}");
}

#[test]
fn token_scan_sees_paths_in_non_expression_macros() {
    let v = json("gaps");
    let layer = v[0]["violations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["rule"] == "layers")
        .expect("layer violation found");
    assert_eq!(
        layer["notes"][0]["message"],
        "`low` uses `crate::high::make`"
    );
}

fn rust_analyzer_available() -> bool {
    Command::new("rust-analyzer")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

#[test]
fn hook_mode_is_silent_on_success() {
    let path = fixture("clean");
    let out = Command::new(env!("CARGO_BIN_EXE_cargo-edict"))
        .args(["edict", "--hook", "--path", path.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty() && out.stderr.is_empty());
}

#[test]
fn hook_mode_exits_2_with_report_on_stderr() {
    let path = fixture("violations");
    let out = Command::new(env!("CARGO_BIN_EXE_cargo-edict"))
        .args(["edict", "--hook", "--path", path.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    assert!(String::from_utf8_lossy(&out.stderr).contains("error[cycles]"));
}

#[test]
fn fast_mode_misses_cycle_through_inferred_method() {
    let path = fixture("inferred");
    let (code, text) = run(&["edict", "--path", path.to_str().unwrap()]);
    assert_eq!(code, 0, "{text}");
}

#[test]
fn full_mode_finds_cycle_through_inferred_method() {
    if !rust_analyzer_available() {
        eprintln!("skipping: rust-analyzer not installed");
        return;
    }
    let path = fixture("inferred");
    let (code, text) = run(&["edict", "--full", "--path", path.to_str().unwrap()]);
    assert_eq!(code, 1, "{text}");
    assert!(text.contains("module cycle: a -> c -> a"), "{text}");
    assert!(
        text.contains("crate::a::Widget::process (impl in crate::c)"),
        "{text}"
    );
    assert!(
        text.contains("src/a.rs:4"),
        "only the non-test call counts: {text}"
    );
    assert!(
        !text.contains("src/a.rs:11"),
        "test code is ignored: {text}"
    );
}

#[test]
fn full_mode_is_a_superset_of_fast_mode() {
    if !rust_analyzer_available() {
        eprintln!("skipping: rust-analyzer not installed");
        return;
    }
    for name in ["clean", "violations", "gaps"] {
        let path = fixture(name);
        let fast = json(name);
        let (_, text) = run(&[
            "edict",
            "--full",
            "--format",
            "json",
            "--path",
            path.to_str().unwrap(),
        ]);
        let full: serde_json::Value = serde_json::from_str(&text).unwrap();
        let msgs = |v: &serde_json::Value| -> Vec<String> {
            v[0]["violations"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x["message"].as_str().unwrap().to_string())
                .collect()
        };
        for m in msgs(&fast) {
            assert!(msgs(&full).contains(&m), "{name}: full mode lost `{m}`");
        }
        assert_eq!(full[0]["mode"], "full");
    }
}

#[test]
fn edict_obeys_its_own_laws_in_full_mode() {
    if !rust_analyzer_available() {
        eprintln!("skipping: rust-analyzer not installed");
        return;
    }
    let (code, text) = run(&["edict", "--full", "--path", env!("CARGO_MANIFEST_DIR")]);
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("0 errors, 0 warnings"), "{text}");
}

#[test]
fn graph_writes_interactive_page() {
    let dir = std::env::temp_dir().join(format!("edict-graph-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("g.html");
    let path = fixture("violations");
    let (code, text) = run(&[
        "edict",
        "graph",
        "--path",
        path.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{text}");
    let html = std::fs::read_to_string(&out).unwrap();
    let start = html.find("const DATA = ").unwrap() + "const DATA = ".len();
    let end = start + html[start..].find(";\n").unwrap();
    let data: serde_json::Value =
        serde_json::from_str(&html[start..end].replace("<\\/", "</")).unwrap();
    let files = &data["views"]["file"];
    let name = |i: &serde_json::Value| {
        files["nodes"][i.as_u64().unwrap() as usize]["name"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let edges = files["edges"].as_array().unwrap();
    let cycle: Vec<(String, String)> = edges
        .iter()
        .filter(|e| e["cycle"] == true)
        .map(|e| (name(&e["from"]), name(&e["to"])))
        .collect();
    assert!(
        cycle.contains(&("billing::invoice".into(), "billing::tax".into())),
        "{cycle:?}"
    );
    assert!(edges.iter().any(|e| e["layerViolation"] == true
        && name(&e["from"]) == "domain"
        && name(&e["to"]) == "infra"));
    assert_eq!(data["layers"].as_array().unwrap().len(), 3);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn submodules_must_stay_private_to_their_top_level_module() {
    let v = json("visibility");
    let msgs: Vec<String> = v[0]["violations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|x| x["rule"] == "facade")
        .map(|x| x["message"].as_str().unwrap().to_string())
        .collect();
    let expect = [
        "`billing::tax` is declared `pub(crate)`, making it visible outside `billing`",
        "`billing::ledger` is declared `pub(super)`, making it visible outside `billing`",
        "`billing::api` is declared `pub`, making it visible outside `billing`",
        "`billing::invoice::deep` is declared `pub(crate)`, making it visible outside `billing`",
    ];
    for e in expect {
        assert!(msgs.iter().any(|m| m == e), "missing `{e}` in {msgs:?}");
    }
    assert_eq!(
        msgs.len(),
        expect.len(),
        "allowed declarations must pass: {msgs:?}"
    );
}

#[test]
fn library_may_opt_in_to_nested_public_modules() {
    let path = fixture("visibility");
    let cfg = path.join("edict-public-api.toml");
    let (_, text) = run(&[
        "edict",
        "--format",
        "json",
        "--path",
        path.to_str().unwrap(),
        "--config",
        cfg.to_str().unwrap(),
    ]);
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    let msgs: Vec<&str> = v[0]["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["message"].as_str().unwrap())
        .collect();
    assert!(!msgs.iter().any(|m| m.contains("billing::api")), "{msgs:?}");
    assert!(
        msgs.iter().any(|m| m.contains("billing::tax")),
        "pub(crate) still fails: {msgs:?}"
    );
}

#[test]
fn parent_child_cycles_fail_by_default() {
    let v = json("parent_child");
    let cycles: Vec<&str> = v[0]["violations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|x| x["rule"] == "cycles")
        .map(|x| x["message"].as_str().unwrap())
        .collect();
    assert_eq!(
        cycles,
        [
            "module cycle: shop -> shop::cart -> shop (3 modules are entangled: shop, shop::cart, shop::stock)"
        ]
    );
}

#[test]
fn allow_parent_child_keeps_sibling_cycles() {
    let path = fixture("parent_child");
    let cfg = path.join("edict-allow.toml");
    let (_, text) = run(&[
        "edict",
        "--format",
        "json",
        "--path",
        path.to_str().unwrap(),
        "--config",
        cfg.to_str().unwrap(),
    ]);
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    let cycles: Vec<&str> = v[0]["violations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|x| x["rule"] == "cycles")
        .map(|x| x["message"].as_str().unwrap())
        .collect();
    assert_eq!(
        cycles,
        ["module cycle: shop::cart -> shop::stock -> shop::cart"],
        "the sibling cycle must still fail"
    );
}
