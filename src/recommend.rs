//! Conservative, explainable cleanup candidates. No candidate authorizes deletion.
use crate::tree::Snapshot;
use serde_json::{Value, json};
use std::{collections::HashMap, path::PathBuf};
const MAX_CANDIDATES: usize = 10_000;
#[derive(Debug)]
pub struct Candidate {
    pub path: PathBuf,
    pub rule: &'static str,
    pub reason: &'static str,
    pub risk: &'static str,
    pub logical: u64,
    pub allocated: u64,
    pub files: u64,
    pub directories: u64,
    pub complete: bool,
}
#[derive(Debug)]
pub struct Report {
    pub candidates: Vec<Candidate>,
    pub total_candidates: u64,
    pub complete: bool,
}
#[derive(Default)]
struct Markers {
    cargo: bool,
    package: bool,
    assets: bool,
    project_settings: bool,
}
fn markers(tree: &Snapshot, parent: u32) -> Markers {
    let mut result = Markers::default();
    for id in tree.child_ids(parent) {
        let node = &tree.nodes[id as usize];
        if node.error.is_some() {
            continue;
        }
        if node.is_dir {
            if node.name == "Assets" {
                result.assets = true;
            }
            if node.name == "ProjectSettings" {
                result.project_settings = true;
            }
        } else if node.files == 1 {
            if node.name == "Cargo.toml" {
                result.cargo = true;
            }
            if node.name == "package.json" {
                result.package = true;
            }
        }
    }
    result
}
pub fn analyze(tree: &Snapshot) -> Report {
    let mut report = Report {
        candidates: Vec::new(),
        total_candidates: 0,
        complete: tree.nodes[0].complete,
    };
    let mut inside_candidate = vec![false; tree.nodes.len()];
    let mut parent_markers: HashMap<u32, Markers> = HashMap::new();
    for (id, node) in tree.nodes.iter().enumerate().skip(1) {
        let parent = node.parent as usize;
        if inside_candidate[parent] {
            inside_candidate[id] = true;
            continue;
        }
        if !node.is_dir || node.error.is_some() {
            continue;
        }
        let name = node.name.as_os_str();
        let needs_markers = name == "target"
            || name == "node_modules"
            || name == ".next"
            || name == "Library"
            || name == "Temp";
        let marker = needs_markers.then(|| {
            parent_markers
                .entry(node.parent)
                .or_insert_with(|| markers(tree, node.parent))
        });
        let rule = match name.to_str() {
            Some("target") if marker.as_ref().is_some_and(|m| m.cargo) => Some((
                "rust_target",
                "Cargo build artifacts beside Cargo.toml",
                "Rebuilt on the next build; may contain custom or unrecoverable files.",
            )),
            Some("node_modules") if marker.as_ref().is_some_and(|m| m.package) => Some((
                "node_modules",
                "Node dependencies beside package.json",
                "Reinstallation may require network access and credentials; may contain local changes.",
            )),
            Some(".next") if marker.as_ref().is_some_and(|m| m.package) => Some((
                "next_build",
                "Next.js build output beside package.json",
                "May contain generated or custom data; a build may be required.",
            )),
            Some("Library")
                if marker
                    .as_ref()
                    .is_some_and(|m| m.assets && m.project_settings) =>
            {
                Some((
                    "unity_library",
                    "Unity project Library beside Assets and ProjectSettings",
                    "Close the Editor first; reimport may take significant time.",
                ))
            }
            Some("Temp")
                if marker
                    .as_ref()
                    .is_some_and(|m| m.assets && m.project_settings) =>
            {
                Some((
                    "unity_temp",
                    "Unity project Temp beside Assets and ProjectSettings",
                    "Close the Editor first; active temporary data may be needed.",
                ))
            }
            Some("__pycache__") => Some((
                "python_bytecode",
                "Python bytecode cache directory",
                "May contain unexpected files; inspect before cleanup.",
            )),
            Some(".pytest_cache") => Some((
                "pytest_cache",
                "pytest cache directory",
                "May contain test state or unexpected files.",
            )),
            Some(".mypy_cache") => Some((
                "mypy_cache",
                "mypy cache directory",
                "May contain analysis state or unexpected files.",
            )),
            Some("caches") if tree.nodes[parent].name == ".gradle" => Some((
                "gradle_caches",
                "Gradle caches directory",
                "Rebuilding may require network access and significant time.",
            )),
            _ => None,
        };
        if let Some((rule, reason, risk)) = rule {
            inside_candidate[id] = true;
            report.total_candidates += 1;
            if report.candidates.len() < MAX_CANDIDATES {
                report.candidates.push(Candidate {
                    path: tree.path(id as u32),
                    rule,
                    reason,
                    risk,
                    logical: node.logical,
                    allocated: node.allocated,
                    files: node.files,
                    directories: node.directories,
                    complete: tree.nodes[0].complete && node.complete,
                });
            }
        }
    }
    report
        .candidates
        .sort_by(|a, b| b.logical.cmp(&a.logical).then_with(|| a.path.cmp(&b.path)));
    report
}
impl Report {
    pub fn json(&self) -> Value {
        let candidates: Vec<_> = self.candidates.iter().map(|c| json!({"path":c.path.to_string_lossy(),"path_units":crate::tree::raw_units(c.path.as_os_str()),"rule":c.rule,"reason":c.reason,"risk":c.risk,"logical_bytes":c.logical,"allocated_attributed_bytes":c.allocated,"files":c.files,"directories":c.directories,"complete":c.complete})).collect();
        json!({"candidates":candidates,"total_candidates":self.total_candidates,"truncated":self.total_candidates > self.candidates.len() as u64,"complete":self.complete,"name_encoding":if cfg!(windows){"utf16"}else{"bytes"},"note":"Heuristics only; inspect each directory. No automatic selection or deletion. Allocated bytes are attributed within the scan and are not guaranteed reclaimed space."})
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn incomplete_tree_is_reflected_in_recommendations() {
        let root = std::env::temp_dir().join(format!(
            "filetree233-recommend-incomplete-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("__pycache__")).unwrap();
        std::fs::write(root.join("__pycache__/file"), b"demo").unwrap();
        let (stop, progress) = crate::tree::new_controls();
        let mut tree = crate::tree::scan(&root, 2, &stop, &progress).unwrap();
        tree.nodes[0].complete = false;
        let report = analyze(&tree);
        assert!(!report.complete);
        assert_eq!(report.candidates.len(), 1);
        assert!(!report.candidates[0].complete);
        assert!(!report.json()["complete"].as_bool().unwrap());
        std::fs::remove_dir_all(root).unwrap();
    }
}
