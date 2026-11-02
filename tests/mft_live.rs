#![cfg(windows)]
use serde_json::Value;
use std::{fs, process::Command};
#[test]
fn live_ntfs_volume_finds_created_demo_file() {
    if std::env::var_os("FILETREE233_TEST_LIVE_MFT").is_none() {
        return;
    }
    let root = std::env::temp_dir();
    let name = format!(
        "filetree233-mft-live-{}-{}.bin",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let file = root.join(&name);
    fs::write(&file, vec![42; 32768]).unwrap();
    let volume_root = file.ancestors().last().unwrap();
    let modes: &[bool] = if std::env::var_os("FILETREE233_TEST_FAST_ONLY").is_some() {
        &[true]
    } else {
        &[false, true]
    };
    for &fast in modes {
        let start = std::time::Instant::now();
        let mut command = Command::new(env!("CARGO_BIN_EXE_filetree233"));
        command
            .arg("mft-volume")
            .arg(volume_root)
            .arg("--name")
            .arg(&name);
        if fast {
            command.arg("--fast");
        }
        let result = command.output().unwrap();
        eprintln!(
            "live NTFS scan mode={} elapsed_ms={}",
            if fast { "fast" } else { "detailed" },
            start.elapsed().as_millis()
        );
        assert!(
            result.status.success(),
            "live MFT inspection failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let rows: Vec<Value> = result
            .stdout
            .split(|b| *b == b'\n')
            .filter(|x| !x.is_empty())
            .map(|x| serde_json::from_slice(x).unwrap())
            .collect();
        let entry = rows
            .iter()
            .find(|row| row["type"] == "record" && row["name"] == name)
            .expect("created demo file absent from MFT");
        assert_eq!(entry["logical_bytes"], 32768);
        assert_eq!(
            rows.last().unwrap()["source"],
            if fast {
                "live_ntfs_volume_fast"
            } else {
                "live_ntfs_volume"
            }
        );
        assert_eq!(rows.last().unwrap()["complete"], false);
        if fast {
            assert!(rows.last().unwrap()["invalid_records"].is_null());
        }
    }
    fs::remove_file(&file).unwrap();
    let demo = root.join(format!("filetree233-mft-tree-{}", std::process::id()));
    fs::create_dir_all(demo.join("alpha/nested")).unwrap();
    fs::create_dir_all(demo.join("beta")).unwrap();
    fs::create_dir_all(demo.join("empty")).unwrap();
    fs::write(demo.join("alpha/nested/data"), vec![1u8; 32768]).unwrap();
    fs::hard_link(demo.join("alpha/nested/data"), demo.join("beta/hardlink")).unwrap();
    fs::write(demo.join("root-file"), vec![2u8; 8192]).unwrap();
    let scan = |engine: &str| {
        let start = std::time::Instant::now();
        let result = Command::new(env!("CARGO_BIN_EXE_filetree233"))
            .arg("scan")
            .arg(&demo)
            .args(["--json", "--engine", engine])
            .output()
            .unwrap();
        eprintln!(
            "demo tree scan engine={} elapsed_ms={}",
            engine,
            start.elapsed().as_millis()
        );
        assert!(
            result.status.success(),
            "demo tree scan failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        serde_json::from_slice::<Value>(&result.stdout).unwrap()
    };
    let walk = scan("walk");
    let mft = scan("mft");
    assert_eq!(mft["engine"], "mft_fast");
    assert_eq!(mft["complete"], false);
    let row = |v: &Value, name: &str| {
        v["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["path"].as_str().is_some_and(|p| p.ends_with(name)))
            .cloned()
            .expect("demo entry missing")
    };
    for name in ["alpha", "beta", "empty", "root-file"] {
        assert_eq!(
            row(&walk, name)["bytes"],
            row(&mft, name)["bytes"],
            "logical size mismatch for {name}"
        );
        assert_eq!(
            row(&walk, name)["files"],
            row(&mft, name)["files"],
            "file count mismatch for {name}"
        );
        assert_eq!(
            row(&walk, name)["directories"],
            row(&mft, name)["directories"],
            "directory count mismatch for {name}"
        );
    }
    assert_eq!(walk["total_allocated_bytes"], mft["total_allocated_bytes"]);
    fs::remove_dir_all(demo).unwrap();
}
