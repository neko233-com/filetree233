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
    let result = Command::new(env!("CARGO_BIN_EXE_filetree233"))
        .arg("mft-volume")
        .arg(volume_root)
        .arg("--name")
        .arg(&name)
        .output()
        .unwrap();
    fs::remove_file(&file).unwrap();
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
    assert_eq!(rows.last().unwrap()["source"], "live_ntfs_volume");
    assert_eq!(rows.last().unwrap()["complete"], false);
}
