use serde_json::Value;
use std::{
    fs,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
#[test]
fn benchmark_reports_isolated_scan_measurements() {
    let root = std::env::temp_dir().join(format!(
        "filetree233-benchmark-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(root.join("nested")).unwrap();
    fs::write(root.join("nested/one"), b"abc").unwrap();
    fs::write(root.join("two"), b"12345").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_filetree233"))
        .arg("bench")
        .arg(&root)
        .args(["--threads", "2", "--iterations", "2"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["iterations"], 2);
    assert_eq!(result["threads"], 2);
    assert_eq!(result["requested_threads"], 2);
    let runs = result["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 2);
    for run in runs {
        assert_eq!(run["engine"], "filesystem_walk");
        assert_eq!(run["requested_threads"], 2);
        assert_eq!(run["scan_threads"], 2);
        assert_eq!(run["complete"], true);
        assert_eq!(run["diagnostics"]["error_nodes"], 0);
        assert_eq!(run["diagnostics"]["incomplete_nodes"], 0);
        assert_eq!(run["entries"], 4);
        assert_eq!(run["files"], 2);
        assert_eq!(run["logical_bytes"], 8);
        assert!(run["peak_memory_bytes"].as_u64().unwrap() > 0);
        assert!(run["process_wall_ms"].as_f64().unwrap() >= run["scan_ms"].as_f64().unwrap());
    }
    let mean = (runs[0]["scan_ms"].as_f64().unwrap() + runs[1]["scan_ms"].as_f64().unwrap()) / 2.0;
    assert!((result["scan_median_ms"].as_f64().unwrap() - mean).abs() < 0.00001);
    assert!(
        !Command::new(env!("CARGO_BIN_EXE_filetree233"))
            .arg("bench")
            .arg(&root)
            .args(["--iterations", "0"])
            .output()
            .unwrap()
            .status
            .success()
    );
    fs::remove_dir_all(root).unwrap();
}
