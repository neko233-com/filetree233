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
    use std::os::windows::ffi::OsStringExt;
    let odd_dir_name = std::ffi::OsString::from_wide(&[b'd' as u16, 0xd800]);
    let odd_file_name = std::ffi::OsString::from_wide(&[b'f' as u16, 0xd801]);
    let odd_dir = demo.join(&odd_dir_name);
    fs::create_dir(&odd_dir).unwrap();
    fs::write(odd_dir.join(&odd_file_name), b"raw-name-content").unwrap();

    fs::write(demo.join("alpha/nested/data"), vec![1u8; 32768]).unwrap();
    fs::hard_link(demo.join("alpha/nested/data"), demo.join("beta/hardlink")).unwrap();
    fs::write(demo.join("root-file"), vec![2u8; 8192]).unwrap();
    let external_link_target = root.join(format!(
        "filetree233-external-hardlink-{}",
        std::process::id()
    ));
    fs::write(&external_link_target, b"small resident content").unwrap();
    fs::hard_link(&external_link_target, demo.join("alpha/external-hardlink")).unwrap();
    fs::write(demo.join("alpha").join("nested").join("中文.txt"), b"ok").unwrap();
    let sparse = demo.join("sparse.bin");
    let sparse_file = fs::File::create(&sparse).unwrap();
    let mark = Command::new("fsutil")
        .args(["sparse", "setflag"])
        .arg(&sparse)
        .output()
        .unwrap();
    assert!(mark.status.success(), "cannot create sparse demo file");
    sparse_file.set_len(64 * 1024 * 1024).unwrap();
    drop(sparse_file);
    let compressed = demo.join("compressed.bin");
    fs::write(&compressed, vec![0u8; 128 * 1024]).unwrap();
    let compressed_result = Command::new("compact.exe")
        .args(["/c", "/q"])
        .arg(&compressed)
        .output()
        .unwrap();
    assert!(
        compressed_result.status.success(),
        "cannot create compressed demo file: {}",
        String::from_utf8_lossy(&compressed_result.stderr)
    );
    fs::create_dir(demo.join("outside")).unwrap();
    fs::write(demo.join("outside/keep"), b"outside").unwrap();
    let junction = Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(demo.join("junction"))
        .arg(demo.join("outside"))
        .output()
        .unwrap();
    assert!(junction.status.success(), "cannot create demo junction");
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
    for summary in [&walk, &mft] {
        let odd = summary["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| {
                entry["path_units"]
                    .as_array()
                    .is_some_and(|units| units.iter().any(|unit| unit.as_u64() == Some(0xd800)))
            })
            .expect("raw directory missing from summary");
        assert_eq!(odd["bytes"], 16);
    }

    let row = |v: &Value, name: &str| {
        v["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["path"].as_str().is_some_and(|p| p.ends_with(name)))
            .cloned()
            .expect("demo entry missing")
    };
    for name in [
        "alpha",
        "beta",
        "empty",
        "root-file",
        "sparse.bin",
        "compressed.bin",
        "outside",
        "junction",
    ] {
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
    let full = Command::new(env!("CARGO_BIN_EXE_filetree233"))
        .arg("tree")
        .arg(&demo)
        .args(["--engine", "mft"])
        .output()
        .unwrap();
    assert!(
        full.status.success(),
        "{}",
        String::from_utf8_lossy(&full.stderr)
    );
    let rows: Vec<Value> = String::from_utf8(full.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows[0]["complete"], false);
    let entries = &rows[1..];
    let odd_dir = entries
        .iter()
        .find(|entry| entry["name_units"] == serde_json::json!([b'd' as u16, 0xd800u16]))
        .expect("raw directory missing from MFT tree");
    let odd_file = entries
        .iter()
        .find(|entry| entry["name_units"] == serde_json::json!([b'f' as u16, 0xd801u16]))
        .expect("raw file missing from MFT tree");
    assert_eq!(odd_file["parent"], odd_dir["id"]);
    assert_eq!(odd_file["logical_bytes"], 16);

    for name in [
        "alpha",
        "nested",
        "data",
        "hardlink",
        "中文.txt",
        "empty",
        "junction",
        "sparse.bin",
        "compressed.bin",
        "external-hardlink",
    ] {
        assert!(
            entries.iter().any(|entry| entry["name"] == name),
            "missing {name}"
        );
    }
    assert_eq!(
        rows[0]["logical_bytes"],
        walk["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e["bytes"].as_u64())
            .sum::<u64>()
    );
    assert_eq!(rows[0]["allocated_bytes"], walk["total_allocated_bytes"]);
    fs::remove_dir_all(demo).unwrap();
    fs::remove_file(external_link_target).unwrap();
}
