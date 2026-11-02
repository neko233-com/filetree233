use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
};
static NEXT: AtomicUsize = AtomicUsize::new(0);

fn scenario(mode: &str) {
    let root = std::env::temp_dir().join(format!(
        "filetree233-mcp-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).unwrap();
    let target = root.join("target");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("file"), b"12345").unwrap();
    let plan = root.join("plan.json");
    let exe = env!("CARGO_BIN_EXE_filetree233");
    assert!(
        Command::new(exe)
            .arg("plan")
            .args([
                "--mode",
                if mode == "recycle_decline" {
                    "recycle"
                } else {
                    "permanent"
                }
            ])
            .arg(&target)
            .arg("-o")
            .arg(&plan)
            .output()
            .unwrap()
            .status
            .success()
    );
    let mut proc = Command::new(exe)
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = proc.stdin.take().unwrap();
    let mut output = BufReader::new(proc.stdout.take().unwrap());
    fn send(w: &mut impl Write, v: Value) {
        writeln!(w, "{v}").unwrap();
        w.flush().unwrap();
    }
    fn receive(r: &mut impl BufRead) -> Value {
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }
    send(
        &mut input,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{"elicitation":{}}}}),
    );
    assert_eq!(
        receive(&mut output)["result"]["protocolVersion"],
        "2025-06-18"
    );
    send(
        &mut input,
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"apply","arguments":{"plan":plan}}}),
    );
    let prompt = receive(&mut output);
    assert_eq!(prompt["method"], "elicitation/create");
    assert!(
        prompt["params"]["message"]
            .as_str()
            .unwrap()
            .contains("5 bytes")
    );
    let token = format!(
        "{} {:.12}",
        if mode == "recycle_decline" {
            "RECYCLE"
        } else {
            "DELETE"
        },
        format!("{:x}", Sha256::digest(fs::read(&plan).unwrap()))
    );
    if mode == "changed" {
        fs::write(target.join("new"), b"x").unwrap();
    }
    if mode == "cancel" {
        send(
            &mut input,
            json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":2}}),
        );
        send(
            &mut input,
            json!({"jsonrpc":"2.0","id":prompt["id"],"result":{"action":"accept","content":{"confirmation":token}}}),
        );
        send(&mut input, json!({"jsonrpc":"2.0","id":99,"method":"ping"}));
        assert_eq!(receive(&mut output)["id"], 99);
        assert!(target.exists());
        drop(input);
        assert!(proc.wait().unwrap().success());
        fs::remove_dir_all(root).unwrap();
        return;
    }
    send(
        &mut input,
        json!({"jsonrpc":"2.0","id":prompt["id"],"result":{"action":if mode == "decline" || mode == "recycle_decline" {"decline"} else {"accept"},"content":{"confirmation":token}}}),
    );
    let result = receive(&mut output);
    assert_eq!(result["result"]["isError"], mode != "approve");
    assert_eq!(target.exists(), mode != "approve");
    drop(input);
    assert!(proc.wait().unwrap().success());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn declined_confirmation_does_not_delete() {
    scenario("decline");
}
#[test]
fn declined_recycle_confirmation_keeps_fixture() {
    scenario("recycle_decline");
}
#[test]
fn changed_target_does_not_delete() {
    scenario("changed");
}
#[test]
fn approved_plan_deletes_fixture() {
    scenario("approve");
}
#[test]
fn cancelled_confirmation_cannot_delete_even_if_late_reply_arrives() {
    scenario("cancel");
}

#[test]
fn cancelled_analysis_keeps_server_responsive_and_releases_slot() {
    let root = std::env::temp_dir().join(format!(
        "filetree233-mcp-cancel-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).unwrap();
    for i in 0..4000 {
        fs::write(root.join(format!("file-{i:05}")), b"x").unwrap();
    }
    let mut proc = Command::new(env!("CARGO_BIN_EXE_filetree233"))
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = proc.stdin.take().unwrap();
    let mut output = BufReader::new(proc.stdout.take().unwrap());
    let export_path = root.with_extension("jsonl");
    for name in ["scan", "tree", "search", "recommend", "duplicates"] {
        for message in [
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":{"path":root,"output":export_path,"query":"file","threads":1}}}),
            json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}}),
            json!({"jsonrpc":"2.0","id":2,"method":"ping"}),
        ] {
            writeln!(input, "{message}").unwrap();
        }
        input.flush().unwrap();
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        let ping: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(ping["id"], 2, "cancelled job must not precede ping: {ping}");
        let mut recovered = false;
        for id in 10..110 {
            writeln!(input, "{}", json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"search","arguments":{"path":root,"query":"file-00000","threads":1}}})).unwrap();
            input.flush().unwrap();
            line.clear();
            output.read_line(&mut line).unwrap();
            let response: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(response["id"], id);
            if response["result"]["isError"] == false {
                recovered = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(recovered, "cancelled {name} did not release its slot");
    }
    drop(input);
    assert!(proc.wait().unwrap().success());
    assert!(root.join("file-00000").exists());
    assert!(!export_path.exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn mcp_scan_accepts_and_validates_parallelism() {
    let root = std::env::temp_dir().join(format!(
        "filetree233-scan-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("demo"), b"demo").unwrap();
    let mut proc = Command::new(env!("CARGO_BIN_EXE_filetree233"))
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = proc.stdin.take().unwrap();
    let mut output = BufReader::new(proc.stdout.take().unwrap());
    fn call(
        w: &mut impl Write,
        r: &mut impl BufRead,
        id: u8,
        path: &std::path::Path,
        threads: u8,
    ) -> Value {
        writeln!(w, "{}", json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"scan","arguments":{"path":path,"threads":threads}}})).unwrap();
        w.flush().unwrap();
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }
    let result = call(&mut input, &mut output, 1, &root, 4);
    assert_eq!(result["result"]["isError"], false);
    let data: Value =
        serde_json::from_str(result["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(data["engine"], "filesystem_walk");
    assert_eq!(data["entries"][0]["bytes"], 4);
    assert_eq!(
        call(&mut input, &mut output, 2, &root, 0)["result"]["isError"],
        true
    );
    drop(input);
    assert!(proc.wait().unwrap().success());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn mcp_tree_exports_once_and_rejects_output_inside_root() {
    let root = std::env::temp_dir().join(format!(
        "filetree233-mcp-tree-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let target = root.join("target");
    fs::create_dir_all(target.join("nested")).unwrap();
    fs::write(target.join("nested/demo"), b"hello").unwrap();
    let output = root.join("tree.jsonl");
    let mut proc = Command::new(env!("CARGO_BIN_EXE_filetree233"))
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = proc.stdin.take().unwrap();
    let mut reader = BufReader::new(proc.stdout.take().unwrap());
    fn call(
        w: &mut impl Write,
        r: &mut impl BufRead,
        id: u8,
        path: &std::path::Path,
        output: &std::path::Path,
    ) -> Value {
        writeln!(w,"{}",json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"tree","arguments":{"path":path,"output":output}}})).unwrap();
        w.flush().unwrap();
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }
    assert_eq!(
        call(
            &mut input,
            &mut reader,
            1,
            &target,
            &target.join("inside.jsonl")
        )["result"]["isError"],
        true
    );
    assert_eq!(
        call(&mut input, &mut reader, 2, &target, &output)["result"]["isError"],
        false
    );
    let lines = fs::read_to_string(&output).unwrap();
    let rows: Vec<Value> = lines
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows[0]["logical_bytes"], 5);
    assert!(rows.iter().any(|r| r["name"] == "demo"));
    assert_eq!(
        call(&mut input, &mut reader, 3, &target, &output)["result"]["isError"],
        true
    );
    drop(input);
    assert!(proc.wait().unwrap().success());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn duplicates_cli_and_mcp_report_verified_copies_without_deleting() {
    let root = std::env::temp_dir().join(format!(
        "filetree233-duplicates-integration-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("first"), b"identical").unwrap();
    fs::write(root.join("second"), b"identical").unwrap();
    fs::write(root.join("different"), b"different").unwrap();
    fs::hard_link(root.join("first"), root.join("alias")).unwrap();
    let cli = Command::new(env!("CARGO_BIN_EXE_filetree233"))
        .arg("duplicates")
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        cli.status.success(),
        "{}",
        String::from_utf8_lossy(&cli.stderr)
    );
    let expected: Value = serde_json::from_slice(&cli.stdout).unwrap();
    assert_eq!(expected["complete"], true);
    assert_eq!(expected["groups"].as_array().unwrap().len(), 1);
    assert_eq!(
        expected["groups"][0]["objects"].as_array().unwrap().len(),
        2
    );
    let mut proc = Command::new(env!("CARGO_BIN_EXE_filetree233"))
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = proc.stdin.take().unwrap();
    let mut output = BufReader::new(proc.stdout.take().unwrap());
    writeln!(input, "{}", json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"duplicates","arguments":{"path":root,"threads":2}}})).unwrap();
    input.flush().unwrap();
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    let result: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(result["result"]["isError"], false);
    let actual: Value =
        serde_json::from_str(result["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(actual["groups"].as_array().unwrap().len(), 1);
    assert_eq!(actual["groups"][0]["objects"].as_array().unwrap().len(), 2);
    assert!(
        root.join("first").exists() && root.join("second").exists() && root.join("alias").exists()
    );
    drop(input);
    assert!(proc.wait().unwrap().success());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn mcp_benchmark_reports_measurements_and_rejects_invalid_iterations() {
    let root = std::env::temp_dir().join(format!(
        "filetree233-mcp-benchmark-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("file"), b"demo").unwrap();
    let mut proc = Command::new(env!("CARGO_BIN_EXE_filetree233"))
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = proc.stdin.take().unwrap();
    let mut reader = BufReader::new(proc.stdout.take().unwrap());
    for (id, iterations) in [(1, 1), (2, 0)] {
        writeln!(input, "{}", json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"benchmark","arguments":{"path":root,"iterations":iterations}}})).unwrap();
        input.flush().unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let result: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(result["result"]["isError"], iterations == 0);
        if iterations == 1 {
            let report: Value =
                serde_json::from_str(result["result"]["content"][0]["text"].as_str().unwrap())
                    .unwrap();
            assert_eq!(report["runs"][0]["logical_bytes"], 4);
            assert!(report["runs"][0]["peak_memory_bytes"].as_u64().unwrap() > 0);
        }
    }
    drop(input);
    assert!(proc.wait().unwrap().success());
    fs::remove_dir_all(root).unwrap();
}
