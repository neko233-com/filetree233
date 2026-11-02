mod allocation;
mod gui;
mod mft;
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, BufRead, IsTerminal, Write},
    path::{Path, PathBuf},
};

#[derive(Parser)]
#[command(version, about = "Disk analysis and human-confirmed cleanup")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}
#[derive(Subcommand)]
enum Command {
    /// Open the graphical disk analyzer.
    Gui,
    /// Analyze immediate children of a directory.
    Scan {
        path: PathBuf,
        #[arg(long)]
        json: bool,
        /// Parallel top-level workers (1-32).
        #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u8).range(1..=32))]
        threads: u8,
    },
    /// Inspect an exported logical NTFS MFT stream as JSON Lines.
    MftInspect {
        path: PathBuf,
        /// Source volume MFT record size; no automatic geometry guessing.
        #[arg(long)]
        record_size: usize,
        /// Omit records not marked in use (invalid slots are still reported).
        #[arg(long)]
        used_only: bool,
    },
    /// Inspect raw MFT records on an NTFS volume (requires raw-volume access).
    MftVolume {
        /// The NTFS volume mount point (for example C:\ on Windows).
        mount: PathBuf,
        /// Show only records whose name contains this text; errors are counted in the summary.
        #[arg(long)]
        name: Option<String>,
    },
    /// Create a cleanup plan for explicitly selected files and directories.
    Plan {
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Recheck a plan and ask a human to confirm in a terminal.
    Apply { plan: PathBuf },
    /// Start the MCP server over stdio.
    Serve,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Summary {
    path: PathBuf,
    bytes: u64,
    allocated_attributed_bytes: u64,
    files: u64,
    directories: u64,
    fingerprint: String,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    version: u32,
    targets: Vec<Summary>,
    total_bytes: u64,
    total_allocated_bytes: u64,
}
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn check_target(path: &Path) -> Result<PathBuf, String> {
    let metadata = fs::symlink_metadata(path).map_err(err)?;
    if (!metadata.is_dir() && !metadata.is_file())
        || metadata.file_type().is_symlink()
        || is_reparse(&metadata)
    {
        return Err(format!(
            "not a regular file or directory or is a link: {}",
            path.display()
        ));
    }
    let full = fs::canonicalize(path).map_err(err)?;
    if full.parent().is_none() {
        return Err("volume root cannot be a cleanup target".into());
    }
    Ok(full)
}
#[cfg(windows)]
fn is_reparse(m: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    m.file_attributes() & 0x400 != 0
}
#[cfg(not(windows))]
fn is_reparse(_: &fs::Metadata) -> bool {
    false
}
fn walk(
    path: &Path,
    hasher: &mut Sha256,
    s: &mut Summary,
    seen_links: &mut std::collections::HashSet<allocation::FileId>,
) -> Result<(), String> {
    let m = fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if m.file_type().is_symlink() || is_reparse(&m) {
        return Err(format!(
            "link/reparse point encountered: {}",
            path.display()
        ));
    }
    let relative = path.strip_prefix(&s.path).map_err(err)?;
    let name = relative.to_string_lossy();
    hasher.update((name.len() as u64).to_le_bytes());
    hasher.update(name.as_bytes());
    hasher.update(m.len().to_le_bytes());
    let modified = m
        .modified()
        .map_err(err)?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(err)?
        .as_nanos();
    hasher.update(modified.to_le_bytes());
    if m.is_dir() {
        hasher.update(b"d");
        s.directories += 1;
        let mut children = fs::read_dir(path)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(err)?;
        children.sort_by_key(|e| e.file_name());
        for child in children {
            walk(&child.path(), hasher, s, seen_links)?;
        }
    } else if m.is_file() {
        hasher.update(b"f");
        let allocation = allocation::file_allocation(path, &m)
            .map_err(|e| format!("{}: allocation: {e}", path.display()))?;
        if allocation.shared_id.is_none_or(|id| seen_links.insert(id)) {
            s.allocated_attributed_bytes = s
                .allocated_attributed_bytes
                .checked_add(allocation.bytes)
                .ok_or("allocation overflow")?;
        }
        s.files += 1;
        s.bytes = s.bytes.checked_add(m.len()).ok_or("size overflow")?;
    } else {
        return Err(format!("unsupported filesystem entry: {}", path.display()));
    }
    Ok(())
}
fn summarize(
    path: &Path,
    seen_links: &mut std::collections::HashSet<allocation::FileId>,
) -> Result<Summary, String> {
    let path = check_target(path)?;
    let mut s = Summary {
        path: path.clone(),
        bytes: 0,
        allocated_attributed_bytes: 0,
        files: 0,
        directories: 0,
        fingerprint: String::new(),
    };
    let mut hasher = Sha256::new();
    walk(&path, &mut hasher, &mut s, seen_links)?;
    s.fingerprint = format!("{:x}", hasher.finalize());
    Ok(s)
}
const MAX_SCAN_ERRORS: usize = 50;
#[derive(Default)]
struct ScanStats {
    bytes: u64,
    allocated: u64,
    files: u64,
    directories: u64,
    error_count: u64,
    allocation_errors: u64,
    errors: Vec<String>,
}
impl ScanStats {
    fn error(&mut self, message: String) {
        self.error_count += 1;
        if self.errors.len() < MAX_SCAN_ERRORS {
            self.errors.push(message);
        }
    }
    fn merge(&mut self, other: Self) {
        self.bytes = self.bytes.saturating_add(other.bytes);
        self.allocated = self.allocated.saturating_add(other.allocated);
        self.files = self.files.saturating_add(other.files);
        self.directories = self.directories.saturating_add(other.directories);
        self.error_count = self.error_count.saturating_add(other.error_count);
        self.allocation_errors = self
            .allocation_errors
            .saturating_add(other.allocation_errors);
        self.errors.extend(
            other
                .errors
                .into_iter()
                .take(MAX_SCAN_ERRORS.saturating_sub(self.errors.len())),
        );
    }
    fn into_value(self, path: &Path) -> Value {
        json!({"path":path,"bytes":self.bytes,"allocated_attributed_bytes":self.allocated,"allocation_errors":self.allocation_errors,"files":self.files,"directories":self.directories,"complete":self.error_count == 0,"error_count":self.error_count,"errors":self.errors})
    }
}
fn count_file(
    path: &Path,
    m: &fs::Metadata,
    stats: &mut ScanStats,
    seen: &std::sync::Mutex<std::collections::HashSet<allocation::FileId>>,
) {
    stats.bytes = stats.bytes.saturating_add(m.len());
    stats.files += 1;
    match allocation::file_allocation(path, m) {
        Ok(a) => {
            if a.shared_id
                .is_none_or(|id| seen.lock().expect("link set poisoned").insert(id))
            {
                stats.allocated = stats.allocated.saturating_add(a.bytes);
            }
        }
        Err(e) => {
            stats.allocation_errors += 1;
            stats.error(format!("{}: allocation: {e}", path.display()));
        }
    }
}
fn scan_directory_job(
    path: &Path,
    seen: &std::sync::Mutex<std::collections::HashSet<allocation::FileId>>,
) -> (ScanStats, Vec<PathBuf>) {
    let mut stats = ScanStats::default();
    let mut subdirs = Vec::new();
    let m = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) => {
            stats.error(format!("{}: {e}", path.display()));
            return (stats, subdirs);
        }
    };
    if m.file_type().is_symlink() || is_reparse(&m) {
        stats.error(format!("{}: link/reparse point skipped", path.display()));
        return (stats, subdirs);
    }
    if m.is_file() {
        count_file(path, &m, &mut stats, seen);
        return (stats, subdirs);
    }
    if !m.is_dir() {
        stats.error(format!("{}: unsupported entry", path.display()));
        return (stats, subdirs);
    }
    stats.directories += 1;
    match fs::read_dir(path) {
        Ok(entries) => {
            for entry in entries {
                match entry {
                    Ok(entry) => {
                        let child = entry.path();
                        match entry.file_type() {
                            Ok(t) if t.is_file() => match fs::symlink_metadata(&child) {
                                Ok(m) if !is_reparse(&m) && m.is_file() => {
                                    count_file(&child, &m, &mut stats, seen)
                                }
                                Ok(_) => stats.error(format!(
                                    "{}: link/reparse point skipped",
                                    child.display()
                                )),
                                Err(e) => stats.error(format!("{}: {e}", child.display())),
                            },
                            Ok(_) => subdirs.push(child),
                            Err(e) => stats.error(format!("{}: {e}", child.display())),
                        }
                    }
                    Err(e) => stats.error(format!("{}: {e}", path.display())),
                }
            }
        }
        Err(e) => stats.error(format!("{}: {e}", path.display())),
    }
    (stats, subdirs)
}
struct ScanQueue {
    jobs: std::collections::VecDeque<(usize, PathBuf)>,
    active: usize,
    totals: Vec<ScanStats>,
}
#[cfg(test)]
fn scan(path: &Path) -> Result<Value, String> {
    scan_with_threads(path, 1)
}
fn scan_with_threads(path: &Path, threads: u8) -> Result<Value, String> {
    if !(1..=32).contains(&threads) {
        return Err("threads must be between 1 and 32".into());
    }
    let root = fs::canonicalize(path).map_err(err)?;
    if !root.is_dir() {
        return Err("scan path must be a directory".into());
    }
    let mut children = Vec::new();
    let mut entries = Vec::new();
    for entry in fs::read_dir(&root).map_err(err)? {
        match entry {
            Ok(entry) => children.push(entry.path()),
            Err(e) => entries.push(json!({"error":e.to_string(),"complete":false})),
        }
    }
    children.sort();
    let seen = std::sync::Mutex::new(std::collections::HashSet::new());
    let queue = std::sync::Mutex::new(ScanQueue {
        jobs: children.iter().cloned().enumerate().collect(),
        active: 0,
        totals: (0..children.len()).map(|_| ScanStats::default()).collect(),
    });
    let wake = std::sync::Condvar::new();
    let worker_count = if children.is_empty() {
        0
    } else {
        usize::from(threads)
    };
    std::thread::scope(|scope| {
        for _ in 0..worker_count {
            scope.spawn(|| {
                loop {
                    let (index, path) = {
                        let mut q = queue.lock().expect("scan queue poisoned");
                        loop {
                            if let Some(job) = q.jobs.pop_front() {
                                q.active += 1;
                                break job;
                            }
                            if q.active == 0 {
                                return;
                            }
                            q = wake.wait(q).expect("scan queue poisoned");
                        }
                    };
                    let (stats, subdirs) = scan_directory_job(&path, &seen);
                    let mut q = queue.lock().expect("scan queue poisoned");
                    q.totals[index].merge(stats);
                    q.jobs.extend(subdirs.into_iter().map(|path| (index, path)));
                    q.active -= 1;
                    wake.notify_all();
                }
            });
        }
    });
    let result = queue.into_inner().map_err(err)?;
    entries.extend(
        result
            .totals
            .into_iter()
            .zip(&children)
            .map(|(stats, path)| stats.into_value(path)),
    );
    entries.sort_by(|a, b| {
        b["bytes"]
            .as_u64()
            .unwrap_or(0)
            .cmp(&a["bytes"].as_u64().unwrap_or(0))
    });
    let total_allocated_bytes: u64 = entries
        .iter()
        .filter_map(|v| v["allocated_attributed_bytes"].as_u64())
        .fold(0u64, u64::saturating_add);
    Ok(
        json!({"root":root,"engine":"filesystem_walk","threads":worker_count,"size_kind":"logical_bytes","allocation_kind":"unique_file_allocation_attributed_first_seen","total_allocated_bytes":total_allocated_bytes,"complete":entries.iter().all(|v| v["complete"] == true),"entries":entries}),
    )
}
fn make_plan(paths: &[PathBuf], output: &Path) -> Result<Plan, String> {
    if paths.is_empty() {
        return Err("at least one target required".into());
    }
    let output_parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let output_full = fs::canonicalize(output_parent)
        .map_err(err)?
        .join(output.file_name().ok_or("invalid plan path")?);
    let cwd = fs::canonicalize(std::env::current_dir().map_err(err)?).map_err(err)?;
    let mut targets = Vec::new();
    let mut seen_links = std::collections::HashSet::new();
    for p in paths {
        let target = check_target(p)?;
        if cwd.starts_with(&target) {
            return Err(format!(
                "target contains current working directory: {}",
                target.display()
            ));
        }
        if output_full.starts_with(&target) {
            return Err("plan file cannot be inside a target".into());
        }
        if targets
            .iter()
            .any(|s: &Summary| target.starts_with(&s.path) || s.path.starts_with(&target))
        {
            return Err("overlapping targets are not allowed".into());
        }
        targets.push(summarize(&target, &mut seen_links)?);
    }
    let total_bytes = targets
        .iter()
        .try_fold(0u64, |a, s| a.checked_add(s.bytes).ok_or("size overflow"))?;
    let total_allocated_bytes = targets.iter().try_fold(0u64, |a, s| {
        a.checked_add(s.allocated_attributed_bytes)
            .ok_or("allocation overflow")
    })?;
    Ok(Plan {
        version: 2,
        targets,
        total_bytes,
        total_allocated_bytes,
    })
}
fn save_plan(paths: &[PathBuf], output: &Path) -> Result<Value, String> {
    let plan = make_plan(paths, output)?;
    let data = serde_json::to_vec_pretty(&plan).map_err(err)?;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .map_err(err)?;
    f.write_all(&data).map_err(err)?;
    Ok(json!({"plan_file":output,"plan":plan}))
}
fn read_checked_plan(path: &Path) -> Result<(Vec<u8>, Plan), String> {
    let data = fs::read(path).map_err(err)?;
    let plan: Plan = serde_json::from_slice(&data).map_err(err)?;
    if plan.version != 2 || plan.targets.is_empty() {
        return Err("invalid plan version or empty plan".into());
    }
    let paths: Vec<_> = plan.targets.iter().map(|s| s.path.clone()).collect();
    let current = make_plan(&paths, path)?;
    if current.targets != plan.targets
        || current.total_bytes != plan.total_bytes
        || current.total_allocated_bytes != plan.total_allocated_bytes
    {
        return Err("plan differs from current filesystem; create a new plan".into());
    }
    Ok((data, plan))
}
fn plan_description(data: &[u8], plan: &Plan) -> (String, String) {
    let digest = format!("{:x}", Sha256::digest(data));
    let token = format!("DELETE {}", &digest[..12]);
    let mut message = String::from(
        "Permanent deletion; logical and attributed file allocation bytes, not guaranteed reclaimed space:\n",
    );
    for s in &plan.targets {
        message.push_str(&format!(
            "{} | {} bytes logical | {} bytes allocated | {} files | {} directories\n",
            s.path.display(),
            s.bytes,
            s.allocated_attributed_bytes,
            s.files,
            s.directories
        ));
    }
    message.push_str(&format!(
        "Total: {} bytes logical | {} bytes allocated\nType {} to confirm.",
        plan.total_bytes, plan.total_allocated_bytes, token
    ));
    (message, token)
}
fn delete_checked(path: &Path, expected_data: &[u8]) -> Result<Value, String> {
    let (data, plan) = read_checked_plan(path)?;
    if data != expected_data {
        return Err("plan file changed during confirmation; nothing deleted".into());
    }
    let mut deleted = Vec::new();
    for s in &plan.targets {
        let metadata = fs::symlink_metadata(&s.path).map_err(err)?;
        if metadata.file_type().is_symlink() || is_reparse(&metadata) {
            return Err(format!("target became a link: {}", s.path.display()));
        }
        let result = if metadata.is_dir() {
            fs::remove_dir_all(&s.path)
        } else if metadata.is_file() {
            fs::remove_file(&s.path)
        } else {
            return Err(format!("unsupported target: {}", s.path.display()));
        };
        if let Err(e) = result {
            return Err(format!(
                "failed deleting {}: {e}; previously deleted: {:?}",
                s.path.display(),
                deleted
            ));
        }
        deleted.push(s.path.clone());
    }
    Ok(
        json!({"deleted":deleted,"planned_logical_bytes":plan.total_bytes,"planned_allocated_attributed_bytes":plan.total_allocated_bytes}),
    )
}
fn apply(path: &Path) -> Result<(), String> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("cleanup requires an interactive terminal".into());
    }
    let (data, plan) = read_checked_plan(path)?;
    let (message, token) = plan_description(&data, &plan);
    println!("{message}");
    let mut response = String::new();
    io::stdin().read_line(&mut response).map_err(err)?;
    if response.trim() != token {
        return Err("confirmation did not match; nothing deleted".into());
    }
    println!("{}", delete_checked(path, &data)?);
    Ok(())
}
fn tool_result(id: &Value, result: Result<Value, String>) -> Value {
    let (v, failed) = match result {
        Ok(v) => (v, false),
        Err(e) => (json!({"error":e}), true),
    };
    json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":v.to_string()}],"isError":failed}})
}
fn serve() -> Result<(), String> {
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    let mut supports_elicitation = false;
    let mut pending: Option<(Value, PathBuf, Vec<u8>, String, String)> = None;
    let mut sequence = 0u64;
    for line in stdin.lock().lines() {
        let line = line.map_err(err)?;
        let req: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => {
                writeln!(stdout, "{}", json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}})).map_err(err)?;
                continue;
            }
        };
        if req.get("method").is_none() {
            if let Some((_, _, _, _, request_id)) = &pending
                && req["id"].as_str() == Some(request_id)
            {
                let (call_id, path, data, token, _) = pending.take().unwrap();
                let approved = req["result"]["action"] == "accept"
                    && req["result"]["content"]["confirmation"].as_str() == Some(token.as_str());
                let result = if approved {
                    delete_checked(&path, &data)
                } else {
                    Err("confirmation declined or invalid; nothing deleted".into())
                };
                writeln!(stdout, "{}", tool_result(&call_id, result)).map_err(err)?;
                stdout.flush().map_err(err)?;
            }
            continue;
        }
        if req["method"] == "notifications/cancelled" {
            if pending
                .as_ref()
                .is_some_and(|(id, _, _, _, _)| *id == req["params"]["requestId"])
            {
                pending = None;
            }
            continue;
        }
        let Some(id) = req.get("id") else {
            continue;
        };
        let method = req["method"].as_str().unwrap_or("");
        let response = match method {
            "initialize" => {
                let e = &req["params"]["capabilities"]["elicitation"];
                supports_elicitation = e.is_object()
                    && (e.as_object().is_some_and(|o| o.is_empty()) || e.get("form").is_some());
                json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"filetree233","version":env!("CARGO_PKG_VERSION")}}})
            }
            "ping" => json!({"jsonrpc":"2.0","id":id,"result":{}}),
            "tools/list" => json!({"jsonrpc":"2.0","id":id,"result":{"tools":[
                {"name":"scan","description":"Analyze immediate children with optional parallel workers, reporting logical bytes, attributed allocation and errors.","inputSchema":{"type":"object","properties":{"path":{"type":"string"},"threads":{"type":"integer","minimum":1,"maximum":32}},"required":["path"]}},
                {"name":"plan","description":"Create a cleanup plan for explicitly selected files and directories; no deletion.","inputSchema":{"type":"object","properties":{"paths":{"type":"array","items":{"type":"string"},"minItems":1},"output":{"type":"string"}},"required":["paths","output"]}},
                {"name":"apply","description":"Permanently delete a previously planned set of files and directories after client-mediated human confirmation and revalidation.","annotations":{"destructiveHint":true,"openWorldHint":false},"inputSchema":{"type":"object","properties":{"plan":{"type":"string"}},"required":["plan"]}}
            ]}}),
            "tools/call" => {
                let args = &req["params"]["arguments"];
                let name = req["params"]["name"].as_str().unwrap_or("");
                if name == "apply" && supports_elicitation && pending.is_none() {
                    let prepared = args["plan"]
                        .as_str()
                        .ok_or("plan required".to_string())
                        .and_then(|p| {
                            let path = PathBuf::from(p);
                            let (data, plan) = read_checked_plan(&path)?;
                            Ok((path, data, plan))
                        });
                    match prepared {
                        Ok((path, data, plan)) => {
                            let (message, token) = plan_description(&data, &plan);
                            sequence += 1;
                            let request_id = format!("filetree-confirm-{sequence}");
                            let request = json!({"jsonrpc":"2.0","id":request_id,"method":"elicitation/create","params":{"message":message,"requestedSchema":{"type":"object","properties":{"confirmation":{"type":"string","title":"Confirmation"}},"required":["confirmation"]}}});
                            pending = Some((id.clone(), path, data, token, request_id));
                            writeln!(stdout, "{request}").map_err(err)?;
                            stdout.flush().map_err(err)?;
                            continue;
                        }
                        Err(e) => tool_result(id, Err(e)),
                    }
                } else {
                    let result = match name {
                        "scan" => (|| { let path = args["path"].as_str().ok_or("path required")?; let threads = if args.get("threads").is_some() { u8::try_from(args["threads"].as_u64().ok_or("invalid threads")?).map_err(|_| "invalid threads")? } else { 4 }; scan_with_threads(Path::new(path), threads) })(),
                        "plan" => (|| { let paths = args["paths"].as_array().ok_or("paths required")?.iter().map(|v| v.as_str().map(PathBuf::from).ok_or("invalid path")).collect::<Result<Vec<_>,_>>()?; let out = args["output"].as_str().ok_or("output required")?; save_plan(&paths, Path::new(out)) })(),
                        "apply" => Err("client must support human elicitation, and only one confirmation can be pending".into()),
                        _ => Err("unknown tool".into())
                    };
                    tool_result(id, result)
                }
            }
            _ => {
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Method not found"}})
            }
        };
        writeln!(stdout, "{response}").map_err(err)?;
        stdout.flush().map_err(err)?;
    }
    Ok(())
}
fn main() {
    let result = match Cli::parse().command.unwrap_or(Command::Gui) {
        Command::Scan { path, json, threads } => scan_with_threads(&path, threads).and_then(|v| {
            if json {
                println!("{}", serde_json::to_string_pretty(&v).map_err(err)?);
            } else {
                println!("Logical bytes | Allocated bytes* | path (*hard links attributed once across this scan)");
                if let Some(entries) = v["entries"].as_array() {
                    for entry in entries {
                        let path = entry["path"].as_str().unwrap_or("?");
                        if let Some(bytes) = entry["bytes"].as_u64() {
                            println!(
                                "{bytes:>13} | {:>16} | {path}{}",
                                entry["allocated_attributed_bytes"].as_u64().unwrap_or(0),
                                if entry["complete"] == false {
                                    " [incomplete]"
                                } else {
                                    ""
                                }
                            );
                        } else {
                            println!(
                                "{:>13} | {:>16} | {path}: {}",
                                "ERROR", "-",
                                entry["error"].as_str().unwrap_or("unknown")
                            );
                        }
                    }
                }
            }
            Ok(())
        }),
        Command::MftInspect { path, record_size, used_only } => mft::inspect_stdout(&path, record_size, used_only),
        Command::MftVolume { mount, name } => mft::inspect_volume_stdout(&mount, name.as_deref()),
        Command::Plan { paths, output } => save_plan(&paths, &output)
            .map(|v| println!("{}", serde_json::to_string_pretty(&v).unwrap())),
        Command::Apply { plan } => apply(&plan),
        Command::Serve => serve(),
        Command::Gui => gui::run(),
    };
    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "mcp-filetree-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn plan_tracks_nested_bytes_and_detects_changes() {
        let f = Fixture::new();
        let target = f.0.join("target");
        fs::create_dir_all(target.join("nested")).unwrap();
        fs::write(target.join("a"), b"abc").unwrap();
        fs::write(target.join("nested/b"), b"12345").unwrap();
        let out = f.0.join("plan.json");
        let old = make_plan(std::slice::from_ref(&target), &out).unwrap();
        assert_eq!(
            (
                old.total_bytes,
                old.targets[0].files,
                old.targets[0].directories
            ),
            (8, 2, 2)
        );
        fs::write(target.join("c"), b"x").unwrap();
        let new = make_plan(&[target], &out).unwrap();
        assert_ne!(old.targets, new.targets);
    }
    #[test]
    fn plan_rejects_overlapping_and_self_contained_plans() {
        let f = Fixture::new();
        let target = f.0.join("a");
        let child = target.join("b");
        fs::create_dir_all(&child).unwrap();
        assert!(make_plan(&[target.clone(), child], &f.0.join("plan.json")).is_err());
        assert!(make_plan(std::slice::from_ref(&target), &target.join("plan.json")).is_err());
    }
    #[test]
    fn file_plan_and_delete_revalidation() {
        let f = Fixture::new();
        let target = f.0.join("item.txt");
        let plan = f.0.join("plan.json");
        fs::write(&target, b"abc").unwrap();
        save_plan(std::slice::from_ref(&target), &plan).unwrap();
        let data = fs::read(&plan).unwrap();
        fs::write(&target, b"different").unwrap();
        assert!(delete_checked(&plan, &data).is_err());
        assert!(target.exists());
        fs::write(&target, b"abc").unwrap();
        fs::remove_file(&plan).unwrap();
        save_plan(std::slice::from_ref(&target), &plan).unwrap();
        delete_checked(&plan, &fs::read(&plan).unwrap()).unwrap();
        assert!(!target.exists());
    }
    #[test]
    fn scan_counts_hard_links_once_across_children() {
        let f = Fixture::new();
        fs::create_dir(f.0.join("a")).unwrap();
        fs::create_dir(f.0.join("b")).unwrap();
        let original = f.0.join("a/original");
        fs::write(&original, vec![42; 32768]).unwrap();
        fs::hard_link(&original, f.0.join("b/link")).unwrap();
        let expected = allocation::file_allocation(&original, &fs::metadata(&original).unwrap())
            .unwrap()
            .bytes;
        let result = scan(&f.0).unwrap();
        assert_eq!(result["complete"], true);
        assert_eq!(result["total_allocated_bytes"], expected);
        let entries = result["entries"].as_array().unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|e| e["bytes"].as_u64().unwrap())
                .sum::<u64>(),
            65536
        );
        assert_eq!(
            entries
                .iter()
                .map(|e| e["allocated_attributed_bytes"].as_u64().unwrap())
                .sum::<u64>(),
            expected
        );
    }
    #[test]
    fn parallel_scan_keeps_logical_counts_and_global_hard_link_allocation() {
        let f = Fixture::new();
        for n in 0..8 {
            fs::create_dir(f.0.join(format!("dir{n}"))).unwrap();
        }
        let original = f.0.join("dir0/original");
        fs::write(&original, vec![7; 32768]).unwrap();
        for n in 1..8 {
            fs::hard_link(&original, f.0.join(format!("dir{n}/link"))).unwrap();
        }
        let serial = scan_with_threads(&f.0, 1).unwrap();
        for _ in 0..4 {
            let parallel = scan_with_threads(&f.0, 4).unwrap();
            assert_eq!(parallel["complete"], true);
            assert_eq!(
                parallel["total_allocated_bytes"],
                serial["total_allocated_bytes"]
            );
            let sum = |v: &Value| {
                v["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|e| e["bytes"].as_u64().unwrap())
                    .sum::<u64>()
            };
            assert_eq!(sum(&parallel), sum(&serial));
        }
        assert!(scan_with_threads(&f.0, 0).is_err());
        assert!(scan_with_threads(&f.0, 33).is_err());
    }
    #[test]
    fn nested_parallel_jobs_contribute_to_one_top_level_entry() {
        let f = Fixture::new();
        let parent = f.0.join("single-root");
        for n in 0..8 {
            let dir = parent.join(format!("branch-{n}/nested"));
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("file"), vec![n as u8; n + 1]).unwrap();
        }
        let serial = scan_with_threads(&f.0, 1).unwrap();
        let parallel = scan_with_threads(&f.0, 4).unwrap();
        assert_eq!(parallel["threads"], 4);
        assert_eq!(parallel["entries"][0]["bytes"], 36);
        assert_eq!(parallel["entries"][0]["files"], 8);
        assert_eq!(parallel["entries"][0]["directories"], 17);
        assert_eq!(
            parallel["entries"][0]["bytes"],
            serial["entries"][0]["bytes"]
        );
        assert_eq!(
            parallel["total_allocated_bytes"],
            serial["total_allocated_bytes"]
        );
    }
    #[test]
    fn sparse_demo_reports_allocated_below_logical() {
        let f = Fixture::new();
        let path = f.0.join("sparse-雪.bin");
        let file = fs::File::create(&path).unwrap();
        #[cfg(windows)]
        {
            let result = std::process::Command::new("fsutil")
                .args(["sparse", "setflag"])
                .arg(&path)
                .output()
                .expect("fsutil is required for the Windows sparse-file test");
            assert!(
                result.status.success(),
                "cannot set sparse flag: {}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        file.set_len(64 * 1024 * 1024).unwrap();
        drop(file);
        let result = scan(&f.0).unwrap();
        let e = &result["entries"][0];
        assert_eq!(e["bytes"], 64 * 1024 * 1024);
        assert_eq!(e["complete"], true);
        assert!(e["allocated_attributed_bytes"].as_u64().unwrap() < e["bytes"].as_u64().unwrap());
        let plan = make_plan(std::slice::from_ref(&path), &f.0.join("plan.json")).unwrap();
        assert_eq!(plan.total_bytes, e["bytes"]);
        assert_eq!(plan.total_allocated_bytes, e["allocated_attributed_bytes"]);
    }
    #[test]
    fn cleanup_plan_deduplicates_hardlinks_across_targets() {
        let f = Fixture::new();
        let original = f.0.join("a");
        let link = f.0.join("b");
        fs::write(&original, vec![1; 32768]).unwrap();
        fs::hard_link(&original, &link).unwrap();
        let one = allocation::file_allocation(&original, &fs::metadata(&original).unwrap())
            .unwrap()
            .bytes;
        let plan = make_plan(&[original, link], &f.0.join("plan.json")).unwrap();
        assert_eq!(plan.total_bytes, 65536);
        assert_eq!(plan.total_allocated_bytes, one);
    }
    #[test]
    fn scan_caps_errors_and_retains_valid_files() {
        let f = Fixture::new();
        let dir = f.0.join("data");
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("ok"), b"123").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            for i in 0..55 {
                symlink("missing", dir.join(format!("link-{i}"))).unwrap();
            }
            let result = scan(&f.0).unwrap();
            let e = &result["entries"][0];
            assert_eq!(e["bytes"], 3);
            assert_eq!(e["complete"], false);
            assert_eq!(e["error_count"], 55);
            assert_eq!(e["errors"].as_array().unwrap().len(), MAX_SCAN_ERRORS);
        }
        #[cfg(windows)]
        {
            let result = scan(&f.0).unwrap();
            assert_eq!(result["entries"][0]["bytes"], 3);
        }
    }
    #[test]
    fn demo_reparse_point_is_skipped_and_cannot_be_planned() {
        let f = Fixture::new();
        let target = f.0.join("outside");
        let root = f.0.join("data");
        fs::create_dir(&target).unwrap();
        fs::create_dir(&root).unwrap();
        fs::write(target.join("do-not-follow"), b"external").unwrap();
        fs::write(root.join("inside"), b"demo").unwrap();
        let link = root.join("junction");
        #[cfg(windows)]
        {
            let status = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(&link)
                .arg(&target)
                .output()
                .unwrap();
            assert!(
                status.status.success(),
                "junction creation failed: {}",
                String::from_utf8_lossy(&status.stderr)
            );
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let result = scan_with_threads(&root, 4).unwrap();
        assert_eq!(result["complete"], false);
        let e = result["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["path"].as_str().unwrap().ends_with("junction"))
            .unwrap();
        assert_eq!(e["complete"], false);
        assert_eq!(e["bytes"], 0);
        assert!(make_plan(std::slice::from_ref(&root), &f.0.join("plan.json")).is_err());
        assert!(target.join("do-not-follow").exists());
    }
    #[test]
    fn does_not_overwrite_plan() {
        let f = Fixture::new();
        let target = f.0.join("a");
        fs::create_dir(&target).unwrap();
        let out = f.0.join("plan.json");
        fs::write(&out, "keep").unwrap();
        assert!(save_plan(&[target], &out).is_err());
        assert_eq!(fs::read_to_string(out).unwrap(), "keep");
    }
}
