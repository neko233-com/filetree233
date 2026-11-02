//! Aggregates NTFS record references without retaining one allocation per file.
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::path::Path;

#[derive(Clone)]
pub struct Directory {
    pub id: u64,
    pub parent: u64,
    pub name: OsString,
    pub reparse: bool,
}
#[derive(Clone)]
pub struct Link {
    pub parent: u64,
    pub name: OsString,
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
    names: HashMap<u64, OsString>,
    rows: BTreeMap<OsString, Total>,
    pub skipped: u64,
    allocation_unknown: u64,
    snapshot: Option<TreeBuilder>,
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
            allocation_unknown: 0,
            snapshot: None,
        }
    }
    pub fn enable_snapshot(&mut self) {
        self.snapshot = Some(TreeBuilder::new(self.root));
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
        for id in &ids {
            let id = *id;
            if let Some(top) = self.owner(id)
                && let Some(name) = self.names.get(&top)
            {
                self.rows.entry(name.clone()).or_default().directories += 1;
            }
        }
        if self.snapshot.is_some() {
            let mut tree = self.snapshot.take().unwrap();
            for id in ids {
                tree.add_directory_chain(id, &self.dirs);
            }
            self.snapshot = Some(tree);
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
    pub fn file_path(&mut self, root_path: &Path, links: &[Link]) -> Option<std::path::PathBuf> {
        for link in links {
            if link.parent == self.root {
                return Some(root_path.join(&link.name));
            }
            if self.owner(link.parent).is_none() {
                continue;
            }
            let mut parts = Vec::new();
            let mut cursor = link.parent;
            while cursor != self.root {
                let dir = self.dirs.get(&cursor)?;
                if dir.reparse {
                    return None;
                }
                parts.push(dir.name.clone());
                cursor = dir.parent;
            }
            let mut path = root_path.to_path_buf();
            for part in parts.into_iter().rev() {
                path.push(part);
            }
            path.push(&link.name);
            return Some(path);
        }
        None
    }
    pub fn note_allocation_unknown(&mut self) {
        self.allocation_unknown += 1;
    }
    pub fn add_file(&mut self, links: &[Link], logical: u64, allocated: u64) {
        if let Some(tree) = &mut self.snapshot {
            tree.add_file(links, logical, allocated);
        }
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
    pub fn finish_snapshot(mut self, root_path: &Path) -> crate::tree::Snapshot {
        self.snapshot
            .take()
            .expect("snapshot enabled")
            .finish(root_path)
    }
    pub fn finish(self, root_path: &Path) -> Value {
        let mut entries: Vec<_> = self.rows.into_iter().map(|(name,t)| {
            let path = root_path.join(name);
            json!({"path":path.to_string_lossy(),"path_units":crate::tree::raw_units(path.as_os_str()),"bytes":t.logical,"allocated_attributed_bytes":t.allocated,"files":t.files,"directories":t.directories,"complete":false,"error_count":1,"errors":[if t.reparse { "MFT reparse point skipped" } else { "MFT scan is best effort; verify with filesystem traversal" }]})
        }).collect();
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
        json!({"root":root_path.to_string_lossy(),"root_units":crate::tree::raw_units(root_path.as_os_str()),"name_encoding":if cfg!(windows){"utf16"}else{"bytes"},"engine":"mft_fast","threads":1,"size_kind":"logical_bytes","allocation_kind":"unique_file_allocation_attributed_first_seen","total_allocated_bytes":total,"complete":false,"unresolved_cycles":self.skipped,"allocation_unknown":self.allocation_unknown,"entries":entries})
    }
}

struct TreeBuilder {
    nodes: Vec<crate::tree::Node>,
    ids: HashMap<u64, u32>,
}
impl TreeBuilder {
    fn new(root: u64) -> Self {
        let mut node = crate::tree::Node::new(Default::default(), crate::tree::NONE);
        node.is_dir = true;
        node.directories = 1;
        node.complete = false;
        node.error =
            Some("Experimental MFT scan may omit records; verify with filesystem traversal".into());
        Self {
            nodes: vec![node],
            ids: HashMap::from([(root, 0)]),
        }
    }
    fn add_node(
        &mut self,
        parent: u32,
        name: &OsStr,
        is_dir: bool,
        reparse: bool,
        logical: u64,
        allocated: u64,
    ) -> u32 {
        let mut node = crate::tree::Node::new(name.into(), parent);
        node.is_dir = is_dir;
        node.logical = logical;
        node.allocated = allocated;
        node.files = u64::from(!is_dir);
        node.directories = u64::from(is_dir);
        if reparse {
            node.complete = false;
            node.error = Some("MFT reparse point skipped".into());
        }
        node.next_sibling = self.nodes[parent as usize].first_child;
        let id = self.nodes.len() as u32;
        self.nodes[parent as usize].first_child = id;
        self.nodes.push(node);
        id
    }
    fn add_directory_chain(&mut self, id: u64, dirs: &HashMap<u64, Directory>) {
        let mut chain = Vec::new();
        let mut seen = HashSet::new();
        let mut cursor = id;
        while !self.ids.contains_key(&cursor) {
            if !seen.insert(cursor) {
                return;
            }
            let Some(d) = dirs.get(&cursor) else {
                return;
            };
            chain.push(cursor);
            cursor = d.parent;
        }
        let mut parent = self.ids[&cursor];
        for id in chain.into_iter().rev() {
            if self.nodes[parent as usize].error.as_deref() == Some("MFT reparse point skipped") {
                return;
            }
            let d = &dirs[&id];
            parent = self.add_node(parent, &d.name, true, d.reparse, 0, 0);
            self.ids.insert(id, parent);
        }
    }
    fn add_file(&mut self, links: &[Link], logical: u64, allocated: u64) {
        let mut assigned = false;
        for link in links {
            let Some(&parent) = self.ids.get(&link.parent) else {
                continue;
            };
            if self.nodes[parent as usize].error.as_deref() == Some("MFT reparse point skipped") {
                continue;
            }
            self.add_node(
                parent,
                &link.name,
                false,
                false,
                logical,
                if assigned { 0 } else { allocated },
            );
            assigned = true;
        }
    }
    fn finish(self, root: &Path) -> crate::tree::Snapshot {
        let mut snapshot = crate::tree::finalize(root.to_path_buf(), self.nodes);
        snapshot.engine = "mft_fast";
        snapshot
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
        assert_eq!(
            idx.file_path(Path::new("/demo"), &[link(11, "x")]),
            Some(Path::new("/demo").join("a").join("sub").join("x"))
        );
        assert_eq!(idx.file_path(Path::new("/demo"), &[link(31, "no")]), None);
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
    #[test]
    fn builds_browsable_snapshot_with_hardlinks_and_skipped_junction() {
        let mut idx = Index::new(5);
        idx.enable_snapshot();
        for d in [
            dir(11, 10, "nested", false),
            dir(10, 5, "alpha", false),
            dir(20, 5, "beta", false),
            dir(30, 5, "junction", true),
            dir(31, 30, "hidden", false),
            dir(40, 41, "cycle", false),
            dir(41, 40, "cycle2", false),
        ] {
            idx.add_directory(d);
        }
        idx.finish_directories();
        idx.add_file(&[link(11, "one"), link(20, "alias")], 100, 128);
        idx.add_file(&[link(31, "not-visible")], 999, 1024);
        let tree = idx.finish_snapshot(Path::new("/demo"));
        assert!(!tree.nodes[0].complete);
        assert_eq!(tree.nodes[0].logical, 200);
        assert_eq!(tree.nodes[0].allocated, 128);
        assert_eq!(tree.nodes[0].files, 2);
        let paths: Vec<_> = (0..tree.nodes.len())
            .map(|id| tree.path(id as u32))
            .collect();
        assert!(paths.contains(&Path::new("/demo").join("alpha/nested/one")));
        assert!(paths.contains(&Path::new("/demo").join("beta/alias")));
        assert!(paths.contains(&Path::new("/demo").join("junction")));
        assert!(
            !paths
                .iter()
                .any(|p| p.ends_with("not-visible") || p.ends_with("cycle"))
        );
    }
    #[test]
    fn retains_raw_file_and_directory_names_in_tree_and_summary() {
        #[cfg(unix)]
        let (dirname, filename) = {
            use std::os::unix::ffi::OsStringExt;
            (
                OsString::from_vec(vec![b'd', 0x80]),
                OsString::from_vec(vec![b'f', 0x81]),
            )
        };
        #[cfg(windows)]
        let (dirname, filename) = {
            use std::os::windows::ffi::OsStringExt;
            (
                OsString::from_wide(&[b'd' as u16, 0xd800]),
                OsString::from_wide(&[b'f' as u16, 0xd801]),
            )
        };
        let mut idx = Index::new(5);
        idx.enable_snapshot();
        idx.add_directory(Directory {
            id: 10,
            parent: 5,
            name: dirname.clone(),
            reparse: false,
        });
        idx.finish_directories();
        idx.add_file(
            &[Link {
                parent: 10,
                name: filename.clone(),
            }],
            9,
            16,
        );
        let json = idx.finish_snapshot(Path::new("/demo"));
        let dir = json.nodes.iter().position(|n| n.name == dirname).unwrap();
        let file = json.nodes.iter().position(|n| n.name == filename).unwrap();
        assert_eq!(json.nodes[file].parent, dir as u32);
        assert_eq!(json.nodes[0].logical, 9);
        let mut summary = Index::new(5);
        summary.add_directory(Directory {
            id: 10,
            parent: 5,
            name: dirname.clone(),
            reparse: false,
        });
        summary.finish_directories();
        summary.add_file(
            &[Link {
                parent: 10,
                name: filename,
            }],
            9,
            16,
        );
        let result = summary.finish(Path::new("/demo"));
        let row = &result["entries"][0];
        assert_eq!(
            row["path_units"],
            serde_json::json!(crate::tree::raw_units(
                Path::new("/demo").join(dirname).as_os_str()
            ))
        );
    }
}
