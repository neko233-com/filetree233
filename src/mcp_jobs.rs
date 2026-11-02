//! Bounded asynchronous MCP analysis jobs.
use crate::{
    CleanupMode, ScanEngine, duplicates, native_path, recommend, save_plan_controlled,
    scan_dispatch_controlled, scan_full_tree, search, tree,
};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub fn run(
    name: &str,
    args: &Value,
    stop: &AtomicBool,
    progress: &AtomicU64,
) -> Result<Value, String> {
    if name == "plan" {
        let paths = args["paths"]
            .as_array()
            .ok_or("paths required")?
            .iter()
            .map(native_path::from_value)
            .collect::<Result<Vec<_>, _>>()?;
        let output = native_path::from_value(&args["output"])?;
        let mode = match args.get("mode") {
            None => CleanupMode::Recycle,
            Some(Value::String(s)) if s == "recycle" => CleanupMode::Recycle,
            Some(Value::String(s)) if s == "permanent" => CleanupMode::Permanent,
            _ => return Err("invalid cleanup mode".into()),
        };
        return save_plan_controlled(&paths, &output, mode, stop);
    }
    let path = native_path::from_value(&args["path"])?;
    let number = |key: &str, default: u64| -> Result<u64, &str> {
        match args.get(key) {
            None => Ok(default),
            Some(v) => v.as_u64().ok_or("invalid number"),
        }
    };
    let threads = u8::try_from(number("threads", 4)?).map_err(|_| "invalid threads")?;
    let engine = match args.get("engine") {
        None => ScanEngine::Walk,
        Some(Value::String(s)) if s == "walk" => ScanEngine::Walk,
        Some(Value::String(s)) if s == "mft" => ScanEngine::Mft,
        Some(Value::String(s)) if s == "auto" => ScanEngine::Auto,
        _ => return Err("invalid engine".into()),
    };
    match name {
        "scan" => scan_dispatch_controlled(&path, threads, engine, stop),
        "tree" => {
            let output = native_path::from_value(&args["output"])?;
            let format = match args.get("format") {
                None => tree::ExportFormat::Jsonl,
                Some(Value::String(s)) if s == "jsonl" => tree::ExportFormat::Jsonl,
                Some(Value::String(s)) if s == "csv" => tree::ExportFormat::Csv,
                _ => return Err("invalid export format".into()),
            };
            tree::save_tree_controlled(&path, &output, threads, engine, format, stop, progress)
        }
        "search" => {
            let flag = |key: &str| -> Result<bool, &str> {
                match args.get(key) {
                    None => Ok(false),
                    Some(v) => v.as_bool().ok_or("invalid boolean"),
                }
            };
            let kind = match args.get("kind") {
                None => search::Kind::All,
                Some(Value::String(s)) if s == "all" => search::Kind::All,
                Some(Value::String(s)) if s == "file" => search::Kind::File,
                Some(Value::String(s)) if s == "directory" => search::Kind::Directory,
                _ => return Err("invalid kind".into()),
            };
            let options = search::Options {
                query: args["query"].as_str().ok_or("query required")?.to_owned(),
                glob: flag("glob")?,
                path_match: flag("path_match")?,
                kind,
                min_bytes: number("min_bytes", 0)?,
                limit: usize::try_from(number("limit", 100)?).map_err(|_| "invalid limit")?,
            };
            if options.query.is_empty() {
                return Err("search query cannot be empty".into());
            }
            if !(1..=10_000).contains(&options.limit) {
                return Err("invalid limit".into());
            }
            let snapshot = scan_full_tree(&path, threads, engine, stop, progress)?;
            if stop.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            search::find_controlled(&snapshot, &options, stop).map(|report| report.json())
        }
        "recommend" => {
            let snapshot = scan_full_tree(&path, threads, engine, stop, progress)?;
            if stop.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            recommend::analyze_controlled(&snapshot, stop).map(|report| report.json())
        }
        "duplicates" => {
            let snapshot = tree::scan(&path, threads, stop, progress)?;
            progress.store(0, Ordering::Relaxed);
            duplicates::find(&snapshot, threads, stop, progress).map(|report| report.json())
        }
        _ => Err("unknown background tool".into()),
    }
}
