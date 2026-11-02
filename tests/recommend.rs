use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
fn write(root: &Path, path: &str, data: &[u8]) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, data).unwrap();
}
#[test]
fn recommendations_are_explainable_non_overlapping_and_feed_a_plan() {
    let base = std::env::temp_dir().join(format!(
        "filetree233-recommend-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let root = base.join("demo");
    write(&root, "rust/Cargo.toml", b"[package]");
    write(&root, "rust/target/artifact", b"123456");
    write(&root, "rust/target/__pycache__/nested", b"123");
    write(&root, "node/package.json", b"{}");
    write(&root, "node/node_modules/lib/file", b"1234");
    write(&root, "false-target/target/user-data", b"keep");
    write(&root, "false-node/node_modules/user-data", b"keep");
    write(&root, "unity/Assets/scene", b"scene");
    write(&root, "unity/ProjectSettings/settings", b"settings");
    write(&root, "unity/Library/cache", b"12345");
    write(&root, "unity/Temp/temp", b"12");
    write(&root, "python/__pycache__/x.pyc", b"123");
    write(&root, "python/.pytest_cache/state", b"1");
    write(&root, "home/.gradle/caches/data", b"12");
    let output = Command::new(env!("CARGO_BIN_EXE_filetree233"))
        .arg("recommend")
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["complete"], true);
    let candidates = result["candidates"].as_array().unwrap();
    let rules: Vec<&str> = candidates
        .iter()
        .map(|v| v["rule"].as_str().unwrap())
        .collect();
    for rule in [
        "rust_target",
        "node_modules",
        "unity_library",
        "unity_temp",
        "python_bytecode",
        "pytest_cache",
        "gradle_caches",
    ] {
        assert!(rules.contains(&rule), "missing {rule}");
    }
    assert_eq!(rules.len(), 7);
    assert_eq!(
        rules.iter().filter(|&&r| r == "python_bytecode").count(),
        1,
        "nested suggestion must be suppressed"
    );
    let rust = candidates
        .iter()
        .find(|v| v["rule"] == "rust_target")
        .unwrap();
    assert_eq!(rust["logical_bytes"], 9);
    assert!(!rust["risk"].as_str().unwrap().is_empty());
    assert!(root.join("false-target/target/user-data").exists());
    let plan = base.join("plan.json");
    let created = Command::new(env!("CARGO_BIN_EXE_filetree233"))
        .arg("plan")
        .arg(root.join("rust/target"))
        .arg("-o")
        .arg(&plan)
        .output()
        .unwrap();
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let plan_data: Value = serde_json::from_slice(&fs::read(&plan).unwrap()).unwrap();
    assert_eq!(plan_data["total_bytes"], rust["logical_bytes"]);
    assert!(root.join("rust/target/artifact").exists());
    let mut proc = Command::new(env!("CARGO_BIN_EXE_filetree233"))
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = proc.stdin.take().unwrap();
    let mut reader = BufReader::new(proc.stdout.take().unwrap());
    writeln!(input,"{}",json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"recommend","arguments":{"path":root,"threads":2}}})).unwrap();
    input.flush().unwrap();
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let response: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(response["result"]["isError"], false);
    let report: Value =
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(report["total_candidates"], 7);
    drop(input);
    assert!(proc.wait().unwrap().success());
    assert!(root.join("rust/target/artifact").exists());
    fs::remove_dir_all(base).unwrap();
}
