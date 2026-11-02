use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Demo(PathBuf);
impl Demo {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "filetree233-e2e-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("safe/nested")).unwrap();
        fs::create_dir_all(root.join("safe/empty")).unwrap();
        fs::create_dir_all(root.join("outside")).unwrap();
        fs::write(root.join("safe/中文.txt"), b"hello").unwrap();
        fs::write(root.join("safe/nested/a.bin"), vec![7u8; 32768]).unwrap();
        fs::hard_link(
            root.join("safe/nested/a.bin"),
            root.join("safe/hardlink.bin"),
        )
        .unwrap();
        fs::write(root.join("outside/keep"), b"keep").unwrap();
        let sparse_path = root.join("safe/sparse.bin");
        let sparse = fs::File::create(&sparse_path).unwrap();
        #[cfg(windows)]
        {
            let result = Command::new("fsutil")
                .args(["sparse", "setflag"])
                .arg(&sparse_path)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "fsutil sparse: {}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        sparse.set_len(64 * 1024 * 1024).unwrap();
        drop(sparse);
        #[cfg(windows)]
        {
            let result = Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(root.join("safe").join("link"))
                .arg(root.join("outside"))
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "mklink: {}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("outside"), root.join("safe").join("link")).unwrap();
        Self(root)
    }
}
impl Drop for Demo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn run(args: &[&std::ffi::OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_filetree233"))
        .args(args)
        .output()
        .unwrap()
}
fn text(path: &Path) -> &std::ffi::OsStr {
    path.as_os_str()
}
fn scan(path: &Path, workers: &str) -> Value {
    let result = run(&[
        "scan".as_ref(),
        text(path),
        "--json".as_ref(),
        "--threads".as_ref(),
        workers.as_ref(),
    ]);
    assert!(
        result.status.success(),
        "scan: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    serde_json::from_slice(&result.stdout).unwrap()
}
#[test]
fn small_demo_scan_and_cleanup_plan_safety() {
    let demo = Demo::new();
    let safe = demo.0.join("safe");
    let serial = scan(&safe, "1");
    let parallel = scan(&safe, "4");
    assert_eq!(serial["complete"], false);
    assert_eq!(parallel["complete"], false);
    assert_eq!(
        serial["total_allocated_bytes"],
        parallel["total_allocated_bytes"]
    );
    let entries = parallel["entries"].as_array().unwrap();
    let entry = |name: &str| {
        entries
            .iter()
            .find(|e| e["path"].as_str().is_some_and(|s| s.ends_with(name)))
            .unwrap()
    };
    assert_eq!(entry("中文.txt")["bytes"], 5);
    assert_eq!(entry("empty")["directories"], 1);
    assert_eq!(entry("link")["complete"], false);
    assert_eq!(entry("sparse.bin")["bytes"], 64 * 1024 * 1024);
    assert!(
        entry("sparse.bin")["allocated_attributed_bytes"]
            .as_u64()
            .unwrap()
            < 64 * 1024 * 1024
    );
    let allocated_one = scan(&safe.join("nested"), "1")["total_allocated_bytes"]
        .as_u64()
        .unwrap();
    assert_eq!(
        entry("nested")["allocated_attributed_bytes"]
            .as_u64()
            .unwrap()
            + entry("hardlink.bin")["allocated_attributed_bytes"]
                .as_u64()
                .unwrap(),
        allocated_one
    );
    let tree_out = run(&["tree".as_ref(), text(&safe)]);
    assert!(
        tree_out.status.success(),
        "tree: {}",
        String::from_utf8_lossy(&tree_out.stderr)
    );
    let tree_rows: Vec<Value> = tree_out
        .stdout
        .split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect();
    assert_eq!(tree_rows[0]["type"], "tree");
    assert_eq!(tree_rows[0]["complete"], false);
    assert!(
        tree_rows
            .iter()
            .any(|row| row["name"] == "中文.txt" && row["logical_bytes"] == 5)
    );
    assert!(
        tree_rows
            .iter()
            .any(|row| row["name"] == "link" && row["complete"] == false)
    );
    assert!(
        tree_rows
            .iter()
            .any(|row| row["name"] == "a.bin" && row["logical_bytes"] == 32768)
    );
    let rejected_plan = demo.0.join("rejected-plan.json");
    let rejected = run(&[
        "plan".as_ref(),
        text(&safe),
        "-o".as_ref(),
        text(&rejected_plan),
    ]);
    assert!(!rejected.status.success());
    assert!(!rejected_plan.exists());
    let plan = demo.0.join("plan.json");
    let target = safe.join("nested");
    let created = run(&["plan".as_ref(), text(&target), "-o".as_ref(), text(&plan)]);
    assert!(
        created.status.success(),
        "plan: {}",
        String::from_utf8_lossy(&created.stderr)
    );
    let data: Value = serde_json::from_slice(&fs::read(&plan).unwrap()).unwrap();
    assert_eq!(data["total_bytes"], 32768);
    assert_eq!(data["total_allocated_bytes"], allocated_one);
    let apply = run(&["apply".as_ref(), text(&plan)]);
    assert!(!apply.status.success());
    assert!(String::from_utf8_lossy(&apply.stderr).contains("interactive terminal"));
    assert!(target.join("a.bin").exists());
    assert!(demo.0.join("outside/keep").exists());
}

#[test]
fn cli_inspects_invalid_mft_slot_without_panicking() {
    let root = std::env::temp_dir().join(format!(
        "filetree233-mft-cli-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).unwrap();
    let file = root.join("empty.mft");
    fs::write(&file, [0u8; 1024]).unwrap();
    let out = run(&[
        "mft-inspect".as_ref(),
        text(&file),
        "--record-size".as_ref(),
        "1024".as_ref(),
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let rows: Vec<Value> = out
        .stdout
        .split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["type"], "invalid_record");
    assert_eq!(rows[1]["invalid_records"], 1);
    assert_eq!(rows[1]["complete"], false);
    fs::remove_file(file).unwrap();
    fs::remove_dir(root).unwrap();
}
