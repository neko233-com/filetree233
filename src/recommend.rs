//! Conservative, explainable cleanup candidates. No candidate authorizes deletion.
use crate::tree::Snapshot;
use serde_json::{Value, json};
use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap},
    path::PathBuf,
};
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
            if node.name.as_ref() == "Assets" {
                result.assets = true;
            }
            if node.name.as_ref() == "ProjectSettings" {
                result.project_settings = true;
            }
        } else if node.files == 1 {
            if node.name.as_ref() == "Cargo.toml" {
                result.cargo = true;
            }
            if node.name.as_ref() == "package.json" {
                result.package = true;
            }
        }
    }
    result
}
pub fn analyze(tree: &Snapshot) -> Report {
    analyze_controlled(tree, &std::sync::atomic::AtomicBool::new(false))
        .expect("uncancelled analysis")
}
pub fn analyze_controlled(
    tree: &Snapshot,
    cancelled: &std::sync::atomic::AtomicBool,
) -> Result<Report, String> {
    let mut report = Report {
        candidates: Vec::new(),
        total_candidates: 0,
        complete: tree.nodes[0].complete,
    };
    let mut inside_candidate = vec![false; tree.nodes.len()];
    let mut parent_markers: HashMap<u32, Markers> = HashMap::new();
    // The smallest retained size (and, on ties, largest path) is at the top.
    // Keep only ids and metadata until the final results are constructed.
    let mut best = BinaryHeap::<
        Reverse<(
            u64,
            Reverse<PathBuf>,
            u32,
            &'static str,
            &'static str,
            &'static str,
        )>,
    >::new();
    for (id, node) in tree.nodes.iter().enumerate().skip(1) {
        if id % 1024 == 0 && cancelled.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("recommendation cancelled".into());
        }
        let parent = node.parent as usize;
        if inside_candidate[parent] {
            inside_candidate[id] = true;
            continue;
        }
        if !node.is_dir || node.error.is_some() {
            continue;
        }
        let name = node.name.as_ref();
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
            Some("caches") if tree.nodes[parent].name.as_ref() == ".gradle" => Some((
                "gradle_caches",
                "Gradle caches directory",
                "Rebuilding may require network access and significant time.",
            )),
            _ => None,
        };
        if let Some((rule, reason, risk)) = rule {
            inside_candidate[id] = true;
            report.total_candidates += 1;
            if best.len() == MAX_CANDIDATES
                && best
                    .peek()
                    .is_some_and(|Reverse(item)| node.logical < item.0)
            {
                continue;
            }
            let item = (
                node.logical,
                Reverse(tree.path(id as u32)),
                id as u32,
                rule,
                reason,
                risk,
            );
            if best.len() < MAX_CANDIDATES {
                best.push(Reverse(item));
            } else if best
                .peek()
                .is_some_and(|Reverse(smallest)| item > *smallest)
            {
                best.pop();
                best.push(Reverse(item));
            }
        }
    }
    report.candidates = best
        .into_iter()
        .map(
            |Reverse((logical, Reverse(path), id, rule, reason, risk))| {
                let node = &tree.nodes[id as usize];
                Candidate {
                    path,
                    rule,
                    reason,
                    risk,
                    logical,
                    allocated: node.allocated,
                    files: node.files,
                    directories: node.directories,
                    complete: tree.nodes[0].complete && node.complete,
                }
            },
        )
        .collect();
    report
        .candidates
        .sort_by(|a, b| b.logical.cmp(&a.logical).then_with(|| a.path.cmp(&b.path)));
    Ok(report)
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
    fn capped_recommendations_keep_largest_and_suppress_nested() {
        let mut root = crate::tree::Node::new(Default::default(), crate::tree::NONE);
        root.is_dir = true;
        root.directories = 1;
        let mut nodes = vec![root];
        for i in 0..MAX_CANDIDATES + 2 {
            let parent_id = nodes.len() as u32;
            let mut parent = crate::tree::Node::new(format!("project-{i:05}").into(), 0);
            parent.is_dir = true;
            parent.directories = 1;
            parent.next_sibling = nodes[0].first_child;
            nodes[0].first_child = parent_id;
            nodes.push(parent);
            let child_id = nodes.len() as u32;
            let mut child = crate::tree::Node::new("__pycache__".into(), parent_id);
            child.is_dir = true;
            child.directories = 1;
            child.logical = if i == MAX_CANDIDATES + 1 { 1000 } else { 1 };
            nodes[parent_id as usize].first_child = child_id;
            nodes.push(child);
            if i == MAX_CANDIDATES + 1 {
                let mut nested = crate::tree::Node::new(".pytest_cache".into(), child_id);
                nested.is_dir = true;
                nested.directories = 1;
                nodes[child_id as usize].first_child = nodes.len() as u32;
                nodes.push(nested);
            }
        }
        let tree = crate::tree::finalize(PathBuf::from("demo"), nodes);
        let report = analyze(&tree);
        assert_eq!(report.total_candidates, (MAX_CANDIDATES + 2) as u64);
        assert_eq!(report.candidates.len(), MAX_CANDIDATES);
        assert_eq!(report.candidates[0].logical, 1000);
        assert!(
            report.candidates[0]
                .path
                .ends_with("project-10001/__pycache__")
        );
        assert!(
            report
                .candidates
                .iter()
                .any(|c| c.path.ends_with("project-00000/__pycache__"))
        );
        assert!(
            !report
                .candidates
                .iter()
                .any(|c| c.path.ends_with("project-10000/__pycache__"))
        );
        assert!(report.candidates.iter().all(|c| c.rule != "pytest_cache"));
        assert_eq!(report.json()["truncated"], true);
    }
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
