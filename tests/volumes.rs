use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
#[test]
fn cli_and_mcp_report_volumes_and_capacity_for_directory_and_file() {
    let root = std::env::temp_dir().join(format!(
        "filetree233-capacity-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("file"), b"demo").unwrap();
    let exe = env!("CARGO_BIN_EXE_filetree233");
    let output = Command::new(exe).arg("volumes").output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let volumes = report["volumes"].as_array().unwrap();
    assert!(!volumes.is_empty(), "system has no detected volumes");
    for volume in volumes {
        assert!(volume["mount"].is_string());
        assert!(volume["mount_units"].is_array());
        assert!(volume["capacity_known"].is_boolean());
    }
    let read_capacity = |path: &std::path::Path| {
        let output = Command::new(exe)
            .arg("capacity")
            .arg(path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };
    let directory = read_capacity(&root);
    let file = read_capacity(&root.join("file"));
    assert!(directory["total_bytes"].as_u64().unwrap() > 0);
    assert_eq!(directory["total_bytes"], file["total_bytes"]);
    assert!(directory["path_units"].is_array());
    assert!(
        !Command::new(exe)
            .arg("capacity")
            .arg(root.join("does-not-exist"))
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
    let mut reader = BufReader::new(proc.stdout.take().unwrap());
    for (id, name, args) in [
        (1, "volumes", json!({})),
        (2, "capacity", json!({"path":root})),
        (3, "capacity", json!({"path":root.join("missing")})),
    ] {
        writeln!(input,"{}",json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":args}})).unwrap();
        input.flush().unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let response: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(response["result"]["isError"], id == 3);
        if id == 2 {
            let result: Value =
                serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap())
                    .unwrap();
            assert_eq!(result["total_bytes"], directory["total_bytes"]);
        }
    }
    drop(input);
    assert!(proc.wait().unwrap().success());
    fs::remove_dir_all(root).unwrap();
}
