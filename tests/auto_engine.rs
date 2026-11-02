use serde_json::Value;
use std::{fs, process::Command};

#[test]
fn auto_uses_walk_consistently_for_subdirectory_operations() {
    let root = std::env::temp_dir().join(format!(
        "filetree233-auto-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(root.join("__pycache__")).unwrap();
    fs::write(root.join("__pycache__/file.log"), b"abc").unwrap();
    let exe = env!("CARGO_BIN_EXE_filetree233");
    let run = |args: &[&str]| {
        let output = Command::new(exe)
            .arg(args[0])
            .arg(&root)
            .args(&args[1..])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    let expected = "auto selects filesystem traversal for subdirectories";
    let scan: Value = serde_json::from_str(&run(&["scan", "--engine", "auto", "--json"])).unwrap();
    assert_eq!(scan["engine"], "filesystem_walk");
    assert_eq!(scan["fallback_reason"], expected);
    let tree = run(&["tree", "--engine", "auto"]);
    let header: Value = serde_json::from_str(tree.lines().next().unwrap()).unwrap();
    assert_eq!(header["engine"], "filesystem_walk");
    assert_eq!(header["fallback_reason"], expected);
    assert_eq!(header["complete"], true);
    let bench: Value =
        serde_json::from_str(&run(&["bench", "--engine", "auto", "--iterations", "1"])).unwrap();
    assert_eq!(bench["runs"][0]["engine"], "filesystem_walk");
    assert_eq!(bench["runs"][0]["fallback_reason"], expected);
    let search: Value =
        serde_json::from_str(&run(&["search", ".log", "--engine", "auto"])).unwrap();
    assert_eq!(search["total_matches"], 1);
    assert_eq!(search["complete"], true);
    let recommend: Value = serde_json::from_str(&run(&["recommend", "--engine", "auto"])).unwrap();
    assert_eq!(recommend["total_candidates"], 1);
    assert_eq!(recommend["complete"], true);
    fs::remove_dir_all(root).unwrap();
}
