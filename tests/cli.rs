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
        .filter(|x| x["rule"] == "facade")
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
