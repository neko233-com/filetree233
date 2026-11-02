//! Bounded, read-only search over an existing tree snapshot.
use crate::tree::Snapshot;
use clap::ValueEnum;
use serde_json::{Value, json};
use std::{cmp::Reverse, collections::BinaryHeap, path::PathBuf};
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Kind {
    All,
    File,
    Directory,
}
#[derive(Clone)]
pub struct Options {
    pub query: String,
    pub glob: bool,
    pub path_match: bool,
    pub kind: Kind,
    pub min_bytes: u64,
    pub limit: usize,
}
#[derive(Debug)]
pub struct Hit {
    pub path: PathBuf,
    pub is_dir: bool,
    pub logical: u64,
    pub allocated: u64,
    pub complete: bool,
}
#[derive(Debug)]
pub struct Report {
    pub hits: Vec<Hit>,
    pub total_matches: u64,
    pub complete: bool,
}
pub fn find(tree: &Snapshot, opts: &Options) -> Result<Report, String> {
    find_controlled(tree, opts, &std::sync::atomic::AtomicBool::new(false))
}
pub fn find_controlled(
    tree: &Snapshot,
    opts: &Options,
    cancelled: &std::sync::atomic::AtomicBool,
) -> Result<Report, String> {
    if opts.query.is_empty() {
        return Err("search query cannot be empty".into());
    }
    if !(1..=10_000).contains(&opts.limit) {
        return Err("search limit must be between 1 and 10000".into());
    }
    let glob = if opts.glob {
        Some(
            globset::GlobBuilder::new(&opts.query)
                .case_insensitive(true)
                .literal_separator(opts.path_match)
                .build()
                .map_err(|e| format!("invalid glob: {e}"))?
                .compile_matcher(),
        )
    } else {
        None
    };
    let needle = opts.query.to_lowercase();
    let mut best = BinaryHeap::<Reverse<(u64, u32)>>::with_capacity(opts.limit);
    let mut total_matches = 0u64;
    for (id, node) in tree.nodes.iter().enumerate().skip(1) {
        if id % 1024 == 0 && cancelled.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("search cancelled".into());
        }
        if node.logical < opts.min_bytes
            || (opts.kind == Kind::File && (node.is_dir || node.files != 1))
            || (opts.kind == Kind::Directory && !node.is_dir)
        {
            continue;
        }
        let path = opts.path_match.then(|| tree.path(id as u32));
        let candidate = path.as_deref().map(|p| p.as_os_str()).unwrap_or(&node.name);
        let matched = if let Some(glob) = &glob {
            glob.is_match(candidate)
        } else {
            candidate.to_string_lossy().to_lowercase().contains(&needle)
        };
        if !matched {
            continue;
        }
        total_matches += 1;
        let item = (node.logical, id as u32);
        if best.len() < opts.limit {
            best.push(Reverse(item));
        } else if best
            .peek()
            .is_some_and(|Reverse(smallest)| item > *smallest)
        {
            best.pop();
            best.push(Reverse(item));
        }
    }
    let mut ids: Vec<_> = best.into_iter().map(|Reverse(item)| item).collect();
    ids.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let hits = ids
        .into_iter()
        .map(|(_, id)| {
            let node = &tree.nodes[id as usize];
            Hit {
                path: tree.path(id),
                is_dir: node.is_dir,
                logical: node.logical,
                allocated: node.allocated,
                complete: tree.nodes[0].complete && node.complete,
            }
        })
        .collect();
    Ok(Report {
        hits,
        total_matches,
        complete: tree.nodes[0].complete,
    })
}
impl Report {
    pub fn json(&self) -> Value {
        let hits:Vec<_>=self.hits.iter().map(|hit|json!({"path":hit.path.to_string_lossy(),"path_units":crate::tree::raw_units(hit.path.as_os_str()),"directory":hit.is_dir,"logical_bytes":hit.logical,"allocated_attributed_bytes":hit.allocated,"complete":hit.complete})).collect();
        json!({"hits":hits,"total_matches":self.total_matches,"truncated":self.total_matches>self.hits.len() as u64,"complete":self.complete,"name_encoding":if cfg!(windows){"utf16"}else{"bytes"},"note":"Largest matches first; path display and substring matching may be lossy for non-Unicode names. Results never authorize cleanup."})
    }
}
