use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
#[test]
fn csv_exports_are_bounded_non_overwriting_and_preserve_raw_names() {
    let base = std::env::temp_dir().join(format!(
        "filetree233-export-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let root = base.join("demo");
    fs::create_dir_all(root.join("nested")).unwrap();
    let filename = "=SUM(1,2).txt";
    fs::write(root.join("nested").join(filename), b"hello").unwrap();
    let exe = env!("CARGO_BIN_EXE_filetree233");
    let result = Command::new(exe)
        .arg("tree")
        .arg(&root)
        .args(["--format", "csv"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let mut reader = csv::Reader::from_reader(result.stdout.as_slice());
    let headers = reader.headers().unwrap().clone();
    let column = |name: &str| headers.iter().position(|v| v == name).unwrap();
    let rows: Vec<_> = reader.records().map(Result::unwrap).collect();
    assert_eq!(rows.len(), 3);
    let file = rows
        .iter()
        .find(|row| {
            row.get(column("name"))
                .is_some_and(|name| name.contains("SUM"))
        })
        .unwrap();
    assert_eq!(&file[column("name")], "'=SUM(1,2).txt");
    assert_eq!(&file[column("spreadsheet_escaped")], "true");
    assert_eq!(&file[column("logical_bytes")], "5");
    let units: Vec<u32> = serde_json::from_str(&file[column("name_units")]).unwrap();
    assert_eq!(
        units,
        filename.encode_utf16().map(u32::from).collect::<Vec<_>>()
    );
    let output = base.join("tree.csv");
    let saved = Command::new(exe)
        .arg("tree")
        .arg(&root)
        .args(["--format", "csv", "--output"])
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        saved.status.success(),
        "{}",
        String::from_utf8_lossy(&saved.stderr)
    );
    let before = fs::read(&output).unwrap();
    let again = Command::new(exe)
        .arg("tree")
        .arg(&root)
        .args(["--format", "csv", "--output"])
        .arg(&output)
        .output()
        .unwrap();
    assert!(!again.status.success());
    assert_eq!(fs::read(&output).unwrap(), before);
    let inside = root.join("inside.csv");
    let rejected = Command::new(exe)
        .arg("tree")
        .arg(&root)
        .args(["--format", "csv", "--output"])
        .arg(&inside)
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert!(!inside.exists());
    let mut proc = Command::new(exe)
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = proc.stdin.take().unwrap();
    let mut pipe = BufReader::new(proc.stdout.take().unwrap());
    let mcp_out = base.join("mcp.csv");
    for (id, format, expected_error) in [(1, "unknown", true), (2, "csv", false)] {
        writeln!(input,"{}",json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"tree","arguments":{"path":root,"output":mcp_out,"format":format}}})).unwrap();
        input.flush().unwrap();
        let mut line = String::new();
        pipe.read_line(&mut line).unwrap();
        let response: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(response["result"]["isError"], expected_error, "{response}");
        if !expected_error {
            let data: Value =
                serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap())
                    .unwrap();
            assert_eq!(data["format"], "csv");
        }
    }
    drop(input);
    assert!(proc.wait().unwrap().success());
    let mut csv = csv::Reader::from_path(&mcp_out).unwrap();
    assert_eq!(csv.records().count(), 3);
    assert!(root.join("nested").join(filename).exists());
    fs::remove_dir_all(base).unwrap();
}
