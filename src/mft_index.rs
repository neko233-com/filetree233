//! Aggregates NTFS record references without retaining one allocation per file.
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

#[derive(Clone)]
pub struct Directory {
    pub id: u64,
    pub parent: u64,
    pub name: String,
    pub reparse: bool,
}
#[derive(Clone)]
pub struct Link {
    pub parent: u64,
    pub name: String,
}
#[derive(Default)]
struct Total {
    logical: u64,
    allocated: u64,
    files: u64,
    directories: u64,
    reparse: bool,
}
pub struct Index {
    root: u64,
    dirs: HashMap<u64, Directory>,
    owners: HashMap<u64, Option<u64>>,
    names: HashMap<u64, String>,
    rows: BTreeMap<String, Total>,
    pub skipped: u64,
}
impl Index {
    pub fn new(root: u64) -> Self {
        Self {
            root,
            dirs: HashMap::new(),
            owners: HashMap::new(),
            names: HashMap::new(),
            rows: BTreeMap::new(),
            skipped: 0,
        }
    }
    pub fn add_directory(&mut self, d: Directory) {
        self.dirs.insert(d.id, d);
    }
    pub fn finish_directories(&mut self) {
        let ids: Vec<u64> = self.dirs.keys().copied().collect();
        for id in &ids {
            let d = &self.dirs[id];
            if d.parent == self.root && !d.name.is_empty() {
                self.names.insert(*id, d.name.clone());
                self.rows.entry(d.name.clone()).or_default().reparse = d.reparse;
            }
        }
        for id in ids {
            if let Some(top) = self.owner(id)
                && let Some(name) = self.names.get(&top)
            {
                self.rows.entry(name.clone()).or_default().directories += 1;
            }
        }
    }
    fn owner(&mut self, id: u64) -> Option<u64> {
        if id == self.root {
            return None;
        }
        if let Some(result) = self.owners.get(&id) {
            return *result;
        }
        let mut chain = Vec::new();
        let mut visited = HashSet::new();
        let mut cursor = id;
        let result = loop {
            if let Some(result) = self.owners.get(&cursor) {
                break *result;
            }
            if !visited.insert(cursor) {
                self.skipped += 1;
                break None;
            }
            let Some(d) = self.dirs.get(&cursor) else {
                break None;
            };
            chain.push(cursor);
            if d.reparse {
                break None;
            }
            if d.parent == self.root {
                break Some(cursor);
            }
            cursor = d.parent;
        };
        for id in chain {
            self.owners.insert(id, result);
        }
        result
    }
    pub fn add_file(&mut self, links: &[Link], logical: u64, allocated: u64) {
        let mut assigned = false;
        for link in links {
            let name = if link.parent == self.root {
                Some(link.name.clone())
            } else {
                self.owner(link.parent)
                    .and_then(|id| self.names.get(&id).cloned())
            };
            if let Some(name) = name {
                let row = self.rows.entry(name).or_default();
                row.logical = row.logical.saturating_add(logical);
                row.files += 1;
                if !assigned {
                    row.allocated = row.allocated.saturating_add(allocated);
                    assigned = true;
                }
            }
        }
    }
    pub fn finish(self, root_path: &Path) -> Value {
        let mut entries: Vec<_> = self.rows.into_iter().map(|(name,t)| json!({"path":root_path.join(name).to_string_lossy(),"bytes":t.logical,"allocated_attributed_bytes":t.allocated,"files":t.files,"directories":t.directories,"complete":false,"error_count":1,"errors":[if t.reparse { "MFT reparse point skipped" } else { "MFT scan is best effort; verify with filesystem traversal" }]})).collect();
        entries.sort_by(|a, b| {
            b["bytes"]
                .as_u64()
                .unwrap_or(0)
                .cmp(&a["bytes"].as_u64().unwrap_or(0))
        });
        let total = entries
            .iter()
            .filter_map(|e| e["allocated_attributed_bytes"].as_u64())
            .fold(0u64, u64::saturating_add);
        json!({"root":root_path.to_string_lossy(),"engine":"mft_fast","threads":1,"size_kind":"logical_bytes","allocation_kind":"unique_file_allocation_attributed_first_seen","total_allocated_bytes":total,"complete":false,"unresolved_cycles":self.skipped,"entries":entries})
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn dir(id: u64, parent: u64, name: &str, reparse: bool) -> Directory {
        Directory {
            id,
            parent,
            name: name.into(),
            reparse,
        }
    }
    fn link(parent: u64, name: &str) -> Link {
        Link {
            parent,
            name: name.into(),
        }
    }
    #[test]
    fn nested_hardlinks_reparse_and_cycles() {
        let mut idx = Index::new(5);
        for d in [
            dir(10, 5, "a", false),
            dir(11, 10, "sub", false),
            dir(20, 5, "b", false),
            dir(30, 5, "junction", true),
            dir(31, 30, "hidden", false),
            dir(40, 41, "cycle", false),
            dir(41, 40, "cycle2", false),
        ] {
            idx.add_directory(d);
        }
        idx.finish_directories();
        idx.add_file(&[link(11, "x"), link(20, "y")], 100, 128);
        idx.add_file(&[link(31, "no")], 999, 1024);
        idx.add_file(&[link(5, "rootfile")], 7, 8);
        let v = idx.finish(Path::new("/demo"));
        let rows = v["entries"].as_array().unwrap();
        let find = |name: &str| {
            rows.iter()
                .find(|v| v["path"].as_str().unwrap().ends_with(name))
                .unwrap()
        };
        assert_eq!(find("a")["bytes"], 100);
        assert_eq!(find("a")["directories"], 2);
        assert_eq!(find("b")["bytes"], 100);
        assert_eq!(find("junction")["files"], 0);
        assert_eq!(v["total_allocated_bytes"], 136);
        assert_eq!(find("rootfile")["bytes"], 7);
        assert!(v["unresolved_cycles"].as_u64().unwrap() > 0);
        assert_eq!(v["complete"], false);
    }
}
