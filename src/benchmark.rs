//! Process-isolated scan measurements; file-system caches are not cleared.
use crate::{ScanEngine, scan_full_tree, tree};
use serde_json::{Value, json};
use std::{path::Path, process::Command, time::Instant};

#[cfg(windows)]
fn peak_memory() -> Result<u64, String> {
    use windows_sys::Win32::System::{
        ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
        Threading::GetCurrentProcess,
    };
    let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    if unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) } == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(counters.PeakWorkingSetSize as u64)
}
#[cfg(unix)]
fn peak_memory() -> Result<u64, String> {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let peak = u64::try_from(usage.ru_maxrss).map_err(|_| "invalid peak RSS")?;
    #[cfg(target_os = "macos")]
    return Ok(peak);
    #[cfg(not(target_os = "macos"))]
    Ok(peak.saturating_mul(1024))
}
#[cfg(not(any(windows, unix)))]
fn peak_memory() -> Result<u64, String> {
    Err("peak memory is unsupported on this platform".into())
}

fn error_counts(snapshot: &tree::Snapshot) -> Value {
    let (mut incomplete, mut links, mut allocation, mut other) = (0u64, 0u64, 0u64, 0u64);
    for node in &snapshot.nodes {
        incomplete += u64::from(!node.complete);
        let Some(error) = node.error.as_deref() else {
            continue;
        };
        if matches!(
            error.as_str(),
            "link/reparse point skipped" | "MFT reparse point skipped"
        ) {
            links += 1;
        } else if error.starts_with("allocation:") || error.starts_with("allocation unavailable:") {
            allocation += 1;
        } else {
            other += 1;
        }
    }
    json!({"incomplete_nodes":incomplete,"error_nodes":links + allocation + other,"skipped_link_nodes":links,"allocation_error_nodes":allocation,"other_error_nodes":other})
}

pub fn once(path: &Path, threads: u8, engine: ScanEngine) -> Result<Value, String> {
    let (cancelled, progress) = tree::new_controls();
    let started = Instant::now();
    let snapshot = scan_full_tree(path, threads, engine, &cancelled, &progress)?;
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    let root = &snapshot.nodes[0];
    let scan_threads = threads;
    Ok(
        json!({"scan_ms":elapsed_ms,"peak_memory_bytes":peak_memory()?,"memory_metric":if cfg!(windows){"peak_working_set"}else{"peak_rss"},"engine":snapshot.engine,"requested_threads":threads,"scan_threads":scan_threads,"fallback_reason":snapshot.fallback_reason,"entries":snapshot.nodes.len(),"files":root.files,"directories":root.directories,"logical_bytes":root.logical,"allocated_bytes":root.allocated,"complete":root.complete,"diagnostics":error_counts(&snapshot)}),
    )
}

pub fn run(path: &Path, threads: u8, engine: ScanEngine, iterations: u8) -> Result<Value, String> {
    if !(1..=32).contains(&threads) || !(1..=10).contains(&iterations) {
        return Err("threads must be 1-32 and iterations must be 1-10".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let engine_name = match engine {
        ScanEngine::Walk => "walk",
        ScanEngine::Mft => "mft",
        ScanEngine::Auto => "auto",
    };
    let mut runs = Vec::with_capacity(iterations as usize);
    for _ in 0..iterations {
        let started = Instant::now();
        let output = Command::new(&exe)
            .arg("bench-once")
            .arg(path)
            .args(["--engine", engine_name, "--threads", &threads.to_string()])
            .output()
            .map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(format!(
                "benchmark worker failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        let mut result: Value =
            serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
        result["process_wall_ms"] = json!(started.elapsed().as_secs_f64() * 1000.0);
        runs.push(result);
    }
    let mut durations: Vec<f64> = runs.iter().filter_map(|r| r["scan_ms"].as_f64()).collect();
    durations.sort_by(f64::total_cmp);
    let median = if durations.len().is_multiple_of(2) {
        (durations[durations.len() / 2 - 1] + durations[durations.len() / 2]) / 2.0
    } else {
        durations[durations.len() / 2]
    };
    Ok(
        json!({"root":path.to_string_lossy(),"root_path":crate::native_path::value(path),"requested_engine":engine_name,"threads":threads,"requested_threads":threads,"iterations":iterations,"scan_median_ms":median,"runs":runs,"note":"Each run is a separate process; filesystem caches are not cleared. Peak memory includes runtime and tree, not just scan allocations. threads is the requested count; see each run's scan_threads for the actual scan worker count."}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagnostics_distinguish_direct_errors_from_incomplete_ancestors() {
        let mut root = tree::Node::new(Default::default(), tree::NONE);
        root.is_dir = true;
        root.complete = false;
        let mut link = tree::Node::new("link".into(), 0);
        link.complete = false;
        link.error = Some(Box::new("link/reparse point skipped".into()));
        let mut allocation = tree::Node::new("file".into(), 0);
        allocation.complete = false;
        allocation.error = Some(Box::new("allocation: access denied".into()));
        let mut other = tree::Node::new("missing".into(), 0);
        other.complete = false;
        other.error = Some(Box::new("permission denied".into()));
        let snapshot = tree::Snapshot {
            root: "demo".into(),
            nodes: vec![root, link, allocation, other],
            engine: "filesystem_walk",
            fallback_reason: None,
        };
        let counts = error_counts(&snapshot);
        assert_eq!(counts["incomplete_nodes"], 4);
        assert_eq!(counts["error_nodes"], 3);
        assert_eq!(counts["skipped_link_nodes"], 1);
        assert_eq!(counts["allocation_error_nodes"], 1);
        assert_eq!(counts["other_error_nodes"], 1);
    }
}
