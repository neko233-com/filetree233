mod gui;
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
fn walk(path: &Path, hasher: &mut Sha256, s: &mut Summary) -> Result<(), String> {
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
            walk(&child.path(), hasher, s)?;
        }
    } else if m.is_file() {
        hasher.update(b"f");
        s.files += 1;
        s.bytes = s.bytes.checked_add(m.len()).ok_or("size overflow")?;
    } else {
        return Err(format!("unsupported filesystem entry: {}", path.display()));
    }
    Ok(())
}
fn summarize(path: &Path) -> Result<Summary, String> {
    let path = check_target(path)?;
    let mut s = Summary {
        path: path.clone(),
        bytes: 0,
        files: 0,
        directories: 0,
        fingerprint: String::new(),
    };
    let mut hasher = Sha256::new();
    walk(&path, &mut hasher, &mut s)?;
    s.fingerprint = format!("{:x}", hasher.finalize());
    Ok(s)
}
fn analyze_entry(path: &Path) -> Value {
    let mut stack = vec![path.to_path_buf()];
    let mut bytes = 0u64;
    let mut files = 0u64;
    let mut directories = 0u64;
    let mut errors = Vec::new();
    while let Some(p) = stack.pop() {
        let m = match fs::symlink_metadata(&p) {
            Ok(m) => m,
            Err(e) => {
                errors.push(format!("{}: {e}", p.display()));
                continue;
            }
        };
        if m.file_type().is_symlink() || is_reparse(&m) {
            errors.push(format!("{}: link/reparse point skipped", p.display()));
            continue;
        }
        if m.is_file() {
            bytes = bytes.saturating_add(m.len());
            files += 1;
        } else if m.is_dir() {
            directories += 1;
            match fs::read_dir(&p) {
                Ok(iter) => {
                    for entry in iter {
                        match entry {
                            Ok(entry) => stack.push(entry.path()),
                            Err(e) => errors.push(format!("{}: {e}", p.display())),
                        }
                    }
                }
                Err(e) => errors.push(format!("{}: {e}", p.display())),
            }
        } else {
            errors.push(format!("{}: unsupported entry", p.display()));
        }
    }
    json!({"path":path,"bytes":bytes,"files":files,"directories":directories,"complete":errors.is_empty(),"errors":errors})
}
fn scan(path: &Path) -> Result<Value, String> {
    let root = fs::canonicalize(path).map_err(err)?;
    if !root.is_dir() {
        return Err("scan path must be a directory".into());
    }
    let mut entries = Vec::new();
    for entry in fs::read_dir(&root).map_err(err)? {
        match entry {
            Ok(entry) => entries.push(analyze_entry(&entry.path())),
            Err(e) => entries.push(json!({"error":e.to_string(),"complete":false})),
        }
    }
    entries.sort_by(|a, b| {
        b["bytes"]
            .as_u64()
            .unwrap_or(0)
            .cmp(&a["bytes"].as_u64().unwrap_or(0))
    });
    Ok(json!({"root":root,"size_kind":"logical_bytes","entries":entries}))
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
        targets.push(summarize(&target)?);
    }
    let total_bytes = targets
        .iter()
        .try_fold(0u64, |a, s| a.checked_add(s.bytes).ok_or("size overflow"))?;
    Ok(Plan {
        version: 1,
        targets,
        total_bytes,
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
    if plan.version != 1 || plan.targets.is_empty() {
        return Err("invalid plan version or empty plan".into());
    }
    let paths: Vec<_> = plan.targets.iter().map(|s| s.path.clone()).collect();
    let current = make_plan(&paths, path)?;
    if current.targets != plan.targets || current.total_bytes != plan.total_bytes {
        return Err("plan differs from current filesystem; create a new plan".into());
    }
    Ok((data, plan))
}
fn plan_description(data: &[u8], plan: &Plan) -> (String, String) {
    let digest = format!("{:x}", Sha256::digest(data));
    let token = format!("DELETE {}", &digest[..12]);
    let mut message =
        String::from("Permanent deletion; logical bytes, not guaranteed reclaimed space:\n");
    for s in &plan.targets {
        message.push_str(&format!(
            "{} | {} bytes | {} files | {} directories\n",
            s.path.display(),
            s.bytes,
            s.files,
            s.directories
        ));
    }
    message.push_str(&format!(
        "Total: {} bytes\nType {} to confirm.",
        plan.total_bytes, token
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
    Ok(json!({"deleted":deleted,"planned_logical_bytes":plan.total_bytes}))
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
            if let Some((_, _, _, _, request_id)) = &pending {
                if req["id"].as_str() == Some(request_id) {
                    let (call_id, path, data, token, _) = pending.take().unwrap();
                    let approved = req["result"]["action"] == "accept"
                        && req["result"]["content"]["confirmation"].as_str()
                            == Some(token.as_str());
                    let result = if approved {
                        delete_checked(&path, &data)
                    } else {
                        Err("confirmation declined or invalid; nothing deleted".into())
                    };
                    writeln!(stdout, "{}", tool_result(&call_id, result)).map_err(err)?;
                    stdout.flush().map_err(err)?;
                }
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
                {"name":"scan","description":"Analyze immediate children, reporting logical bytes and errors.","inputSchema":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}},
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
                        "scan" => args["path"].as_str().ok_or("path required".to_string()).and_then(|p| scan(Path::new(p))),
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
        Command::Scan { path, json } => scan(&path).and_then(|v| {
            if json {
                println!("{}", serde_json::to_string_pretty(&v).map_err(err)?);
            } else {
                println!("Logical bytes | path (incomplete entries report an error)");
                if let Some(entries) = v["entries"].as_array() {
                    for entry in entries {
                        let path = entry["path"].as_str().unwrap_or("?");
                        if let Some(bytes) = entry["bytes"].as_u64() {
                            println!(
                                "{bytes:>13} | {path}{}",
                                if entry["complete"] == false {
                                    " [incomplete]"
                                } else {
                                    ""
                                }
                            );
                        } else {
                            println!(
                                "{:>13} | {path}: {}",
                                "ERROR",
                                entry["error"].as_str().unwrap_or("unknown")
                            );
                        }
                    }
                }
            }
            Ok(())
        }),
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
