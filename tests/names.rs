#![cfg(any(unix, windows))]
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs,
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
#[test]
fn raw_names_round_trip_through_plan_and_confirmed_mcp_cleanup() {
    #[cfg(unix)]
    let name = {
        use std::os::unix::ffi::OsStringExt;
        OsString::from_vec(vec![b'd', 0x80])
    };
    #[cfg(windows)]
    let name = {
        use std::os::windows::ffi::OsStringExt;
        OsString::from_wide(&[b'd' as u16, 0xd800])
    };
    let base = std::env::temp_dir().join(format!(
        "filetree233-raw-name-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let target = base.join(&name);
    if let Err(error) = fs::create_dir_all(&target) {
        #[cfg(target_os = "macos")]
        if error.raw_os_error() == Some(libc::EILSEQ) {
            eprintln!(
                "filesystem rejects non-UTF-8 names; native-name behavior remains covered by synthetic index tests"
            );
            fs::remove_dir_all(&base).unwrap();
            return;
        }
        panic!("failed to create native-name fixture: {error}");
    }
    fs::write(target.join("file"), b"hello").unwrap();
    let exe = env!("CARGO_BIN_EXE_filetree233");
    let out = Command::new(exe)
        .arg("scan")
        .arg(&base)
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let summary: Value = serde_json::from_slice(&out.stdout).unwrap();
    let row = &summary["entries"][0];
    assert_eq!(row["bytes"], 5);
    assert!(!row["path_units"].as_array().unwrap().is_empty());
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        assert!(
            row["path_units"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v.as_u64() == Some(0x80))
        );
        assert!(target.as_os_str().as_bytes().ends_with(&[0x80]));
    }
    #[cfg(windows)]
    assert!(
        row["path_units"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_u64() == Some(0xd800))
    );
    let tree = Command::new(exe).arg("tree").arg(&base).output().unwrap();
    assert!(
        tree.status.success(),
        "{}",
        String::from_utf8_lossy(&tree.stderr)
    );
    let rows: Vec<Value> = String::from_utf8(tree.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    #[cfg(unix)]
    let unit = 0x80;
    #[cfg(windows)]
    let unit = 0xd800;
    assert!(rows.iter().any(|v| {
        v["name_units"]
            .as_array()
            .is_some_and(|a| a.iter().any(|v| v.as_u64() == Some(unit)))
    }));
    let csv = Command::new(exe)
        .arg("tree")
        .arg(&base)
        .args(["--format", "csv"])
        .output()
        .unwrap();
    assert!(
        csv.status.success(),
        "{}",
        String::from_utf8_lossy(&csv.stderr)
    );
    let mut csv_reader = csv::Reader::from_reader(csv.stdout.as_slice());
    let header = csv_reader.headers().unwrap().clone();
    let name_units_index = header.iter().position(|name| name == "name_units").unwrap();
    assert!(csv_reader.records().map(Result::unwrap).any(|record| {
        serde_json::from_str::<Vec<u32>>(&record[name_units_index])
            .unwrap()
            .contains(&(unit as u32))
    }));
    let root_scan = Command::new(exe)
        .arg("scan")
        .arg(&target)
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        root_scan.status.success(),
        "{}",
        String::from_utf8_lossy(&root_scan.stderr)
    );
    let root_report: Value = serde_json::from_slice(&root_scan.stdout).unwrap();
    assert!(
        root_report["root_units"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_u64() == Some(unit))
    );

    let plan = base.join("plan.json");
    let out = Command::new(exe)
        .arg("plan")
        .arg(&target)
        .args(["--mode", "permanent", "-o"])
        .arg(&plan)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let cli: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(cli["plan"]["version"], 5);
    assert!(cli["plan"]["targets"][0]["path"].is_object());
    assert!(target.join("file").exists());
    let mut raw_plan_name = name.clone();
    raw_plan_name.push(".json");
    let raw_plan_path = fs::canonicalize(&base).unwrap().join(raw_plan_name);
    #[cfg(unix)]
    let raw_plan_units: Vec<u32> = {
        use std::os::unix::ffi::OsStrExt;
        raw_plan_path
            .as_os_str()
            .as_bytes()
            .iter()
            .map(|&b| u32::from(b))
            .collect()
    };
    #[cfg(windows)]
    let raw_plan_units: Vec<u32> = {
        use std::os::windows::ffi::OsStrExt;
        raw_plan_path
            .as_os_str()
            .encode_wide()
            .map(u32::from)
            .collect()
    };
    let encoding = if cfg!(windows) { "utf16" } else { "bytes" };
    let raw_target = json!({"encoding":encoding,"units":row["path_units"]});
    let raw_plan = json!({"encoding":encoding,"units":raw_plan_units});
    let mut proc = Command::new(exe)
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = proc.stdin.take().unwrap();
    let mut reader = BufReader::new(proc.stdout.take().unwrap());
    let send = |input: &mut std::process::ChildStdin, value: Value| {
        writeln!(input, "{value}").unwrap();
        input.flush().unwrap();
    };
    let receive = |reader: &mut BufReader<std::process::ChildStdout>| {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        serde_json::from_str::<Value>(&line).unwrap()
    };
    send(
        &mut input,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{"elicitation":{}}}}),
    );
    assert!(receive(&mut reader)["result"].is_object());
    for (id, tool) in [
        (10, "scan"),
        (11, "capacity"),
        (12, "recommend"),
        (13, "duplicates"),
        (14, "benchmark"),
    ] {
        let mut arguments = json!({"path":raw_target});
        if tool == "benchmark" {
            arguments["iterations"] = json!(1);
        }
        send(
            &mut input,
            json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":tool,"arguments":arguments}}),
        );
        let result = receive(&mut reader);
        assert_eq!(result["result"]["isError"], false, "{tool}: {result}");
    }
    let mut raw_tree_name = name.clone();
    raw_tree_name.push(".tree.jsonl");
    let raw_tree_path = fs::canonicalize(&base).unwrap().join(raw_tree_name);
    let mut raw_tree_units = row["path_units"].as_array().unwrap().clone();
    raw_tree_units.extend(".tree.jsonl".bytes().map(|b| json!(b)));
    let raw_tree = json!({"encoding":encoding,"units":raw_tree_units});
    send(
        &mut input,
        json!({"jsonrpc":"2.0","id":15,"method":"tools/call","params":{"name":"tree","arguments":{"path":raw_target,"output":raw_tree}}}),
    );
    let tree_result = receive(&mut reader);
    assert_eq!(tree_result["result"]["isError"], false, "{tree_result}");
    let tree_report: Value = serde_json::from_str(
        tree_result["result"]["content"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(tree_report["output_path"]["encoding"], encoding);
    assert!(
        fs::read_to_string(&raw_tree_path)
            .unwrap()
            .contains("logical_bytes")
    );
    send(
        &mut input,
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"plan","arguments":{"paths":[raw_target],"output":raw_plan,"mode":"permanent"}}}),
    );
    let response = receive(&mut reader);
    assert_eq!(response["result"]["isError"], false, "{response}");
    let created: Value =
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(created["plan_file"]["encoding"], encoding);
    assert!(target.join("file").exists());
    for (id, approve) in [(3, false), (4, true)] {
        send(
            &mut input,
            json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"apply","arguments":{"plan":raw_plan}}}),
        );
        let prompt = receive(&mut reader);
        assert_eq!(prompt["method"], "elicitation/create");
        let message = prompt["params"]["message"].as_str().unwrap();
        assert!(message.contains("encoding") && message.contains(&unit.to_string()));
        let token = format!(
            "DELETE {:.12}",
            format!("{:x}", Sha256::digest(fs::read(&raw_plan_path).unwrap()))
        );
        send(
            &mut input,
            json!({"jsonrpc":"2.0","id":prompt["id"],"result":{"action":if approve {"accept"} else {"decline"},"content":{"confirmation":token}}}),
        );
        let result = receive(&mut reader);
        assert_eq!(result["result"]["isError"], !approve, "{result}");
        if !approve {
            assert!(target.join("file").exists());
        } else {
            let result: Value =
                serde_json::from_str(result["result"]["content"][0]["text"].as_str().unwrap())
                    .unwrap();
            assert_eq!(result["processed"][0]["encoding"], encoding);
        }
    }
    assert!(!target.exists());
    drop(input);
    assert!(proc.wait().unwrap().success());
    fs::remove_dir_all(base).unwrap();
}
