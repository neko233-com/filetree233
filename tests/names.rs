#![cfg(any(unix, windows))]
use serde_json::Value;
use std::{
    ffi::OsString,
    fs,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
#[test]
fn raw_names_are_preserved_and_unserializable_plans_are_rejected() {
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
    fs::create_dir_all(&target).unwrap();
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
        .arg("-o")
        .arg(&plan)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(!plan.exists());
    assert!(target.join("file").exists());
    fs::remove_dir_all(base).unwrap();
}
