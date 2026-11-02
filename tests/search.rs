use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
fn write(root: &Path, path: &str, size: usize) {
    let file = root.join(path);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, vec![b'x'; size]).unwrap();
}
#[test]
fn search_finds_nested_files_filters_and_never_deletes() {
    let base = std::env::temp_dir().join(format!(
        "filetree233-search-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let root = base.join("demo");
    write(&root, "a/nested/One.LOG", 5);
    write(&root, "b/nested/two.log", 12);
    write(&root, "b/nested/three.log", 8);
    write(&root, "b/unrelated.txt", 40);
    let exe = env!("CARGO_BIN_EXE_filetree233");
    let output = Command::new(exe)
        .arg("search")
        .arg(&root)
        .arg(".log")
        .args(["--kind", "file", "--limit", "2", "--min-bytes", "6"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["total_matches"], 2);
    assert_eq!(report["hits"][0]["logical_bytes"], 12);
    assert_eq!(report["hits"][1]["logical_bytes"], 8);
    assert_eq!(report["truncated"], false);
    let glob = Command::new(exe)
        .arg("search")
        .arg(&root)
        .arg("**/nested/*.log")
        .args(["--glob", "--path-match", "--kind", "file", "--limit", "1"])
        .output()
        .unwrap();
    assert!(
        glob.status.success(),
        "{}",
        String::from_utf8_lossy(&glob.stderr)
    );
    let glob: Value = serde_json::from_slice(&glob.stdout).unwrap();
    assert_eq!(glob["total_matches"], 3);
    assert_eq!(glob["hits"][0]["logical_bytes"], 12);
    assert_eq!(glob["truncated"], true);
    let invalid = Command::new(exe)
        .arg("search")
        .arg(&root)
        .arg("[")
        .arg("--glob")
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    let plan = base.join("plan.json");
    let output = Command::new(exe)
        .arg("plan")
        .arg(root.join("b/nested/two.log"))
        .arg("-o")
        .arg(&plan)
        .output()
        .unwrap();
    assert!(output.status.success());
    let plan: Value = serde_json::from_slice(&fs::read(plan).unwrap()).unwrap();
    assert_eq!(plan["total_bytes"], 12);
    assert!(root.join("b/nested/two.log").exists());
    let mut proc = Command::new(exe)
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = proc.stdin.take().unwrap();
    let mut reader = BufReader::new(proc.stdout.take().unwrap());
    for (id, limit) in [(1, 1), (2, 0)] {
        writeln!(input,"{}",json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"search","arguments":{"path":root,"query":"*.log","glob":true,"kind":"file","limit":limit}}})).unwrap();
        input.flush().unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let response: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(response["result"]["isError"], limit == 0);
        if limit == 1 {
            let r: Value =
                serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap())
                    .unwrap();
            assert_eq!(r["total_matches"], 3);
            assert_eq!(r["hits"][0]["logical_bytes"], 12);
        }
    }
    drop(input);
    assert!(proc.wait().unwrap().success());
    assert!(root.join("b/nested/two.log").exists());
    fs::remove_dir_all(base).unwrap();
}
