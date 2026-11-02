//! Compact, in-memory filesystem tree for analysis and export.
use crate::{allocation, is_reparse};
use std::{
    collections::HashSet,
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
pub(crate) const NONE: u32 = u32::MAX;

#[derive(Debug)]
pub struct Node {
    pub name: Box<OsStr>,
    pub parent: u32,
    pub first_child: u32,
    pub next_sibling: u32,
    pub logical: u64,
    pub allocated: u64,
    pub files: u64,
    pub directories: u64,
    pub is_dir: bool,
    // Errors are rare; keeping their String out of line saves space on every
    // successful node in large snapshots.
    #[allow(clippy::box_collection)] // Saves 16 bytes per node; errors are uncommon.
    pub error: Option<Box<String>>,
    pub complete: bool,
}
impl Node {
    pub(crate) fn new(name: OsString, parent: u32) -> Self {
        Self {
            name: name.into_boxed_os_str(),
            parent,
            first_child: NONE,
            next_sibling: NONE,
            logical: 0,
            allocated: 0,
            files: 0,
            directories: 0,
            is_dir: false,
            error: None,
            complete: true,
        }
    }
}
#[derive(Debug)]
pub struct Snapshot {
    pub root: PathBuf,
    pub nodes: Vec<Node>,
    pub engine: &'static str,
    pub fallback_reason: Option<String>,
}
impl Snapshot {
    pub fn path(&self, id: u32) -> PathBuf {
        let mut parts = Vec::new();
        let mut cursor = id;
        while cursor != 0 {
            let n = &self.nodes[cursor as usize];
            parts.push(n.name.as_ref());
            cursor = n.parent;
        }
        let mut path = self.root.clone();
        for part in parts.into_iter().rev() {
            path.push(part);
        }
        path
    }
    pub fn child_ids(&self, id: u32) -> impl Iterator<Item = u32> + '_ {
        let first = self.nodes[id as usize].first_child;
        std::iter::successors((first != NONE).then_some(first), move |id| {
            let next = self.nodes[*id as usize].next_sibling;
            (next != NONE).then_some(next)
        })
    }
    #[cfg(test)]
    pub fn children(&self, id: u32) -> Vec<u32> {
        self.child_ids(id).collect()
    }
}
struct Queue {
    nodes: Vec<Node>,
    jobs: std::collections::VecDeque<(PathBuf, u32)>,
    active: usize,
    error: Option<String>,
}
// Limit temporary memory and publish child directories often enough for other
// workers to start scanning while a large directory is still being enumerated.
const TREE_BATCH_SIZE: usize = 64;
fn publish_batch(
    queue: &std::sync::Mutex<Queue>,
    wake: &std::sync::Condvar,
    progress: &AtomicU64,
    parent: u32,
    batch: &mut Vec<(Node, Option<PathBuf>)>,
) -> bool {
    if batch.is_empty() {
        return true;
    }
    let mut q = queue.lock().expect("tree queue poisoned");
    if q.error.is_some() {
        batch.clear();
        return false;
    }
    let mut new_jobs = 0;
    let count = batch.len() as u64;
    for (mut node, directory) in batch.drain(..) {
        let id = match u32::try_from(q.nodes.len()) {
            Ok(id) if id != NONE => id,
            _ => {
                q.error = Some("tree exceeds supported number of entries".into());
                wake.notify_all();
                return false;
            }
        };
        node.next_sibling = q.nodes[parent as usize].first_child;
        q.nodes[parent as usize].first_child = id;
        q.nodes.push(node);
        if let Some(path) = directory {
            q.jobs.push_back((path, id));
            new_jobs += 1;
        }
    }
    progress.fetch_add(count, Ordering::Relaxed);
    if new_jobs > 0 {
        wake.notify_all();
    }
    true
}
pub fn scan(
    path: &Path,
    threads: u8,
    cancelled: &AtomicBool,
    progress: &AtomicU64,
) -> Result<Snapshot, String> {
    if !(1..=32).contains(&threads) {
        return Err("threads must be between 1 and 32".into());
    }
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_dir() || is_reparse(&meta) {
        return Err("scan root must be a non-link directory".into());
    }
    let root = fs::canonicalize(path).map_err(|e| e.to_string())?;
    let mut first = Node::new(OsString::new(), NONE);
    first.is_dir = true;
    first.directories = 1;
    let queue = std::sync::Mutex::new(Queue {
        nodes: vec![first],
        jobs: std::collections::VecDeque::from([(root.clone(), 0)]),
        active: 0,
        error: None,
    });
    let links = std::sync::Mutex::new(HashSet::new());
    let wake = std::sync::Condvar::new();
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                let mut batch = Vec::with_capacity(TREE_BATCH_SIZE);
                loop {
                    let (dir, parent) = {
                        let mut q = queue.lock().expect("tree queue poisoned");
                        loop {
                            if cancelled.load(Ordering::Relaxed) || q.error.is_some() {
                                wake.notify_all();
                                return;
                            }
                            if let Some(job) = q.jobs.pop_front() {
                                q.active += 1;
                                break job;
                            }
                            if q.active == 0 {
                                return;
                            }
                            q = wake.wait(q).expect("tree queue poisoned");
                        }
                    };
                    match fs::read_dir(&dir) {
                        Ok(listing) => {
                            for item in listing {
                                if cancelled.load(Ordering::Relaxed) {
                                    break;
                                }
                                let entry = match item {
                                    Ok(e) => e,
                                    Err(e) => {
                                        let mut q = queue.lock().expect("tree queue poisoned");
                                        let n = &mut q.nodes[parent as usize];
                                        n.error = Some(Box::new(e.to_string()));
                                        n.complete = false;
                                        continue;
                                    }
                                };
                                let path = entry.path();
                                let mut node = Node::new(entry.file_name(), parent);
                                let mut descend = false;
                                match fs::symlink_metadata(&path) {
                                    Ok(m) if m.file_type().is_symlink() || is_reparse(&m) => {
                                        node.complete = false;
                                        node.error =
                                            Some(Box::new("link/reparse point skipped".into()));
                                    }
                                    Ok(m) if m.is_dir() => {
                                        node.is_dir = true;
                                        node.directories = 1;
                                        descend = true;
                                    }
                                    Ok(m) if m.is_file() => {
                                        node.files = 1;
                                        node.logical = m.len();
                                        match allocation::file_allocation(&path, &m) {
                                            Ok(a) => {
                                                if a.shared_id.is_none_or(|id| {
                                                    links
                                                        .lock()
                                                        .expect("link set poisoned")
                                                        .insert(id)
                                                }) {
                                                    node.allocated = a.bytes;
                                                }
                                            }
                                            Err(e) => {
                                                node.complete = false;
                                                node.error =
                                                    Some(Box::new(format!("allocation: {e}")));
                                            }
                                        }
                                    }
                                    Ok(_) => {
                                        node.complete = false;
                                        node.error = Some(Box::new("unsupported entry".into()));
                                    }
                                    Err(e) => {
                                        node.complete = false;
                                        node.error = Some(Box::new(e.to_string()));
                                    }
                                }
                                batch.push((node, descend.then_some(path)));
                                if batch.len() == TREE_BATCH_SIZE
                                    && !publish_batch(&queue, &wake, progress, parent, &mut batch)
                                {
                                    break;
                                }
                            }
                            publish_batch(&queue, &wake, progress, parent, &mut batch);
                        }
                        Err(e) => {
                            let mut q = queue.lock().expect("tree queue poisoned");
                            let n = &mut q.nodes[parent as usize];
                            n.error = Some(Box::new(e.to_string()));
                            n.complete = false;
                        }
                    }
                    let mut q = queue.lock().expect("tree queue poisoned");
                    q.active -= 1;
                    wake.notify_all();
                }
            });
        }
    });
    if cancelled.load(Ordering::Relaxed) {
        return Err("scan cancelled".into());
    }
    let state = queue.into_inner().map_err(|e| e.to_string())?;
    if let Some(error) = state.error {
        return Err(error);
    }
    Ok(finalize(root, state.nodes))
}

pub(crate) fn finalize(root: PathBuf, mut nodes: Vec<Node>) -> Snapshot {
    for id in (1..nodes.len()).rev() {
        let (parent, logical, allocated, files, directories, complete) = {
            let n = &nodes[id];
            (
                n.parent as usize,
                n.logical,
                n.allocated,
                n.files,
                n.directories,
                n.complete,
            )
        };
        let n = &mut nodes[parent];
        n.logical = n.logical.saturating_add(logical);
        n.allocated = n.allocated.saturating_add(allocated);
        n.files = n.files.saturating_add(files);
        n.directories = n.directories.saturating_add(directories);
        n.complete &= complete;
    }
    Snapshot {
        root,
        nodes,
        engine: "filesystem_walk",
        fallback_reason: None,
    }
}
pub fn new_controls() -> (Arc<AtomicBool>, Arc<AtomicU64>) {
    (
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicU64::new(0)),
    )
}

pub fn raw_units(name: &std::ffi::OsStr) -> Vec<u32> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        name.encode_wide().map(u32::from).collect()
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        name.as_bytes().iter().copied().map(u32::from).collect()
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = name;
        Vec::new()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum ExportFormat {
    Jsonl,
    Csv,
}
fn spreadsheet_safe(text: &str) -> (std::borrow::Cow<'_, str>, bool) {
    let first = text
        .trim_start_matches([' ', '\t', '\r', '\n'])
        .chars()
        .next();
    if text.starts_with(['\t', '\r', '\n'])
        || first.is_some_and(|c| matches!(c, '=' | '+' | '-' | '@'))
    {
        (std::borrow::Cow::Owned(format!("'{text}")), true)
    } else {
        (std::borrow::Cow::Borrowed(text), false)
    }
}
impl Snapshot {
    /// Write a parent-indexed JSONL snapshot without building one giant JSON document.
    pub fn write_export(
        &self,
        out: &mut impl std::io::Write,
        format: ExportFormat,
    ) -> Result<(), String> {
        match format {
            ExportFormat::Jsonl => self.write_jsonl(out),
            ExportFormat::Csv => self.write_csv(out),
        }
    }
    pub fn write_csv(&self, out: &mut impl std::io::Write) -> Result<(), String> {
        let mut writer = csv::Writer::from_writer(out);
        writer
            .write_record([
                "id",
                "parent",
                "path",
                "name",
                "path_units",
                "name_units",
                "name_encoding",
                "directory",
                "logical_bytes",
                "allocated_attributed_bytes",
                "files",
                "directories",
                "complete",
                "error",
                "engine",
                "scan_complete",
                "spreadsheet_escaped",
            ])
            .map_err(|e| e.to_string())?;
        let encoding = if cfg!(windows) { "utf16" } else { "bytes" };
        for (id, node) in self.nodes.iter().enumerate() {
            let path = self.path(id as u32);
            let path_display = path.to_string_lossy();
            let name_display = node.name.to_string_lossy();
            let (safe_path, path_escaped) = spreadsheet_safe(&path_display);
            let (safe_name, name_escaped) = spreadsheet_safe(&name_display);
            let (safe_error, error_escaped) =
                spreadsheet_safe(node.error.as_deref().map(String::as_str).unwrap_or(""));
            let path_units =
                serde_json::to_string(&raw_units(path.as_os_str())).map_err(|e| e.to_string())?;
            let name_units =
                serde_json::to_string(&raw_units(&node.name)).map_err(|e| e.to_string())?;
            let fields = [
                id.to_string(),
                if id == 0 {
                    String::new()
                } else {
                    node.parent.to_string()
                },
                safe_path.into_owned(),
                safe_name.into_owned(),
                path_units,
                name_units,
                encoding.into(),
                node.is_dir.to_string(),
                node.logical.to_string(),
                node.allocated.to_string(),
                node.files.to_string(),
                node.directories.to_string(),
                node.complete.to_string(),
                safe_error.into_owned(),
                self.engine.into(),
                self.nodes[0].complete.to_string(),
                (path_escaped || name_escaped || error_escaped).to_string(),
            ];
            writer.write_record(&fields).map_err(|e| e.to_string())?;
        }
        writer.flush().map_err(|e| e.to_string())
    }
    pub fn write_jsonl(&self, out: &mut impl std::io::Write) -> Result<(), String> {
        use serde_json::json;
        writeln!(out, "{}", json!({"type":"tree","root":self.root.to_string_lossy(),"root_units":raw_units(self.root.as_os_str()),"name_encoding":if cfg!(windows) {"utf16"} else {"bytes"},"entries":self.nodes.len(),"engine":self.engine,"fallback_reason":self.fallback_reason,"complete":self.nodes[0].complete,"logical_bytes":self.nodes[0].logical,"allocated_bytes":self.nodes[0].allocated})).map_err(|e|e.to_string())?;
        for (id, node) in self.nodes.iter().enumerate() {
            let raw_name = raw_units(&node.name);
            writeln!(out,"{}",json!({"type":"entry","id":id,"parent":if id==0 { None } else { Some(node.parent) },"name":node.name.to_string_lossy(),"name_units":raw_name,"name_encoding":if cfg!(windows) {"utf16"} else {"bytes"},"directory":node.is_dir,"logical_bytes":node.logical,"allocated_bytes":node.allocated,"files":node.files,"directories":node.directories,"complete":node.complete,"error":node.error})).map_err(|e|e.to_string())?;
        }
        out.flush().map_err(|e| e.to_string())
    }
}
fn export_destination(root: &Path, output: &Path) -> Result<PathBuf, String> {
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let destination = fs::canonicalize(parent)
        .map_err(|e| e.to_string())?
        .join(output.file_name().ok_or("invalid output path")?);
    if destination.starts_with(root) {
        return Err("tree output cannot be inside the scanned directory".into());
    }
    if fs::symlink_metadata(&destination).is_ok() {
        return Err("tree output already exists".into());
    }
    Ok(destination)
}
struct CancelWriter<'a, W> {
    inner: W,
    cancelled: &'a AtomicBool,
}
impl<W: std::io::Write> std::io::Write for CancelWriter<'_, W> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if self.cancelled.load(Ordering::Relaxed) {
            return Err(std::io::Error::other("export cancelled"));
        }
        self.inner.write(data)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        if self.cancelled.load(Ordering::Relaxed) {
            return Err(std::io::Error::other("export cancelled"));
        }
        self.inner.flush()
    }
}
fn export_snapshot_controlled(
    snapshot: &Snapshot,
    output: &Path,
    format: ExportFormat,
    cancelled: &AtomicBool,
) -> Result<serde_json::Value, String> {
    let destination = export_destination(&snapshot.root, output)?;
    publish_export(&destination, cancelled, |writer| {
        snapshot.write_export(writer, format)
    })?;
    Ok(
        serde_json::json!({"output":destination.to_string_lossy(),"output_path":crate::native_path::value(&destination),"format":match format {ExportFormat::Jsonl=>"jsonl",ExportFormat::Csv=>"csv"},"entries":snapshot.nodes.len(),"engine":snapshot.engine,"fallback_reason":snapshot.fallback_reason,"complete":snapshot.nodes[0].complete,"logical_bytes":snapshot.nodes[0].logical,"allocated_bytes":snapshot.nodes[0].allocated}),
    )
}
fn publish_export(
    destination: &Path,
    cancelled: &AtomicBool,
    write: impl FnOnce(&mut CancelWriter<'_, &mut tempfile::NamedTempFile>) -> Result<(), String>,
) -> Result<(), String> {
    if cancelled.load(Ordering::Relaxed) {
        return Err("export cancelled".into());
    }
    // Stage beside the destination; failed or cancelled exports are removed on drop.
    let mut file = tempfile::Builder::new()
        .prefix(".filetree233-")
        .tempfile_in(destination.parent().ok_or("invalid output parent")?)
        .map_err(|e| e.to_string())?;
    write(&mut CancelWriter {
        inner: &mut file,
        cancelled,
    })?;
    if cancelled.load(Ordering::Relaxed) {
        return Err("export cancelled".into());
    }
    file.persist_noclobber(destination)
        .map_err(|e| e.to_string())?;
    Ok(())
}
pub fn publish_bytes(output: &Path, bytes: &[u8], cancelled: &AtomicBool) -> Result<(), String> {
    use std::io::Write;
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let destination = fs::canonicalize(parent)
        .map_err(|e| e.to_string())?
        .join(output.file_name().ok_or("invalid output path")?);
    publish_export(&destination, cancelled, |writer| {
        writer.write_all(bytes).map_err(|e| e.to_string())
    })
}
pub fn save_tree(
    path: &Path,
    output: &Path,
    threads: u8,
    engine: crate::ScanEngine,
    format: ExportFormat,
) -> Result<serde_json::Value, String> {
    let (stop, progress) = new_controls();
    save_tree_controlled(path, output, threads, engine, format, &stop, &progress)
}
pub fn save_tree_controlled(
    path: &Path,
    output: &Path,
    threads: u8,
    engine: crate::ScanEngine,
    format: ExportFormat,
    stop: &AtomicBool,
    progress: &AtomicU64,
) -> Result<serde_json::Value, String> {
    if stop.load(Ordering::Relaxed) {
        return Err("scan cancelled".into());
    }
    let root = fs::canonicalize(path).map_err(|e| e.to_string())?;
    let _ = export_destination(&root, output)?;
    let snapshot = crate::scan_full_tree(path, threads, engine, stop, progress)?;
    export_snapshot_controlled(&snapshot, output, format, stop)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancelled_or_failed_export_never_publishes_partial_file() {
        use std::io::Write;
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("tree.jsonl");
        let stop = AtomicBool::new(false);
        let result = publish_export(&destination, &stop, |writer| {
            writer.write_all(b"partial").map_err(|e| e.to_string())?;
            stop.store(true, Ordering::Relaxed);
            writer.write_all(b"remaining").map_err(|e| e.to_string())
        });
        assert!(result.is_err());
        assert!(!destination.exists());
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
        stop.store(false, Ordering::Relaxed);
        fs::write(&destination, b"existing").unwrap();
        assert!(
            publish_export(&destination, &stop, |writer| writer
                .write_all(b"new")
                .map_err(|e| e.to_string()))
            .is_err()
        );
        assert_eq!(fs::read(&destination).unwrap(), b"existing");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }
    #[test]
    fn csv_sanitizes_formula_prefixes_and_reports_write_failures() {
        for text in ["=1+1", "  +cmd", "\ttext", "@formula", "-123"] {
            assert!(spreadsheet_safe(text).1, "{text:?}");
        }
        assert!(!spreadsheet_safe("ordinary.txt").1);
        struct Fails;
        impl std::io::Write for Fails {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("fixture failure"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut root = Node::new(OsString::new(), NONE);
        root.is_dir = true;
        let tree = Snapshot {
            root: PathBuf::from("root"),
            nodes: vec![root],
            engine: "filesystem_walk",
            fallback_reason: None,
        };
        assert!(tree.write_csv(&mut Fails).is_err());
    }
    #[test]
    fn jsonl_preserves_raw_name_units() {
        #[cfg(windows)]
        let name = {
            use std::os::windows::ffi::OsStringExt;
            OsString::from_wide(&[0xd800, b'x' as u16])
        };
        #[cfg(unix)]
        let name = {
            use std::os::unix::ffi::OsStringExt;
            OsString::from_vec(vec![0xff, b'x'])
        };
        #[cfg(not(any(windows, unix)))]
        let name = OsString::from("x");
        let mut root = Node::new(OsString::new(), NONE);
        root.is_dir = true;
        root.first_child = 1;
        let tree = Snapshot {
            engine: "filesystem_walk",
            fallback_reason: None,
            root: PathBuf::from("root"),
            nodes: vec![root, Node::new(name, 0)],
        };
        let mut output = Vec::new();
        tree.write_jsonl(&mut output).unwrap();
        let rows: Vec<serde_json::Value> = output
            .split(|b| *b == b'\n')
            .filter(|x| !x.is_empty())
            .map(|x| serde_json::from_slice(x).unwrap())
            .collect();
        #[cfg(windows)]
        assert_eq!(rows[2]["name_units"], serde_json::json!([0xd800, 120]));
        #[cfg(unix)]
        assert_eq!(rows[2]["name_units"], serde_json::json!([255, 120]));
    }
    #[test]
    #[ignore = "explicit in-memory finalization benchmark"]
    fn benchmark_wide_directory_finalization() {
        for _ in 0..3 {
            let count = 250_000u32;
            let mut root = Node::new(OsString::new(), NONE);
            root.is_dir = true;
            root.directories = 1;
            let mut nodes = Vec::with_capacity(count as usize + 1);
            nodes.push(root);
            for id in 1..=count {
                let mut child = Node::new(format!("file-{:08}", id.wrapping_mul(7919)).into(), 0);
                child.files = 1;
                child.logical = u64::from(id.wrapping_mul(7919) % 100_000);
                child.next_sibling = nodes[0].first_child;
                nodes[0].first_child = id;
                nodes.push(child);
            }
            let start = std::time::Instant::now();
            let tree = finalize(PathBuf::from("benchmark"), nodes);
            std::hint::black_box(&tree);
            eprintln!(
                "wide_finalization_us={} nodes={}",
                start.elapsed().as_micros(),
                tree.nodes.len()
            );
            assert_eq!(tree.nodes[0].files, u64::from(count));
        }
    }
    #[test]
    fn batched_scan_preserves_every_child_and_parent() {
        let root = std::env::temp_dir().join(format!(
            "filetree233-tree-batch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        for i in 0..130 {
            let dir = root.join(format!("dir-{i:03}"));
            fs::create_dir(&dir).unwrap();
            fs::write(dir.join("file"), b"hello").unwrap();
        }
        for threads in [1, 8] {
            let (stop, progress) = new_controls();
            let tree = scan(&root, threads, &stop, &progress).unwrap();
            assert_eq!(tree.nodes.len(), 261);
            assert_eq!(tree.nodes[0].directories, 131);
            assert_eq!(tree.nodes[0].files, 130);
            assert_eq!(tree.nodes[0].logical, 650);
            assert_eq!(progress.load(Ordering::Relaxed), 260);
            assert!(tree.nodes[0].complete);
            for dir in tree.children(0) {
                let files = tree.children(dir);
                assert_eq!(files.len(), 1);
                assert_eq!(tree.nodes[files[0] as usize].name.as_ref(), "file");
                assert_eq!(tree.nodes[files[0] as usize].parent, dir);
            }
            assert_eq!(tree.children(0).len(), 130);
        }
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn snapshot_keeps_tree_and_aggregates_hard_links() {
        let root = std::env::temp_dir().join(format!(
            "filetree233-tree-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("a/nested")).unwrap();
        fs::create_dir_all(root.join("b")).unwrap();
        let original = root.join("a/nested/雪.txt");
        fs::write(&original, vec![1; 32768]).unwrap();
        fs::hard_link(&original, root.join("b/link")).unwrap();
        let (stop, progress) = new_controls();
        let tree = scan(&root, 4, &stop, &progress).unwrap();
        let (serial_stop, serial_progress) = new_controls();
        let serial = scan(&root, 1, &serial_stop, &serial_progress).unwrap();
        assert_eq!(tree.nodes[0].logical, serial.nodes[0].logical);
        assert_eq!(tree.nodes[0].allocated, serial.nodes[0].allocated);
        assert_eq!(tree.nodes[0].files, serial.nodes[0].files);
        assert_eq!(tree.nodes[0].directories, serial.nodes[0].directories);
        assert_eq!(tree.nodes[0].logical, 65536);
        assert_eq!(tree.nodes[0].files, 2);
        assert_eq!(tree.nodes[0].directories, 4);
        assert_eq!(progress.load(Ordering::Relaxed), 5);
        let expected = allocation::file_allocation(&original, &fs::metadata(&original).unwrap())
            .unwrap()
            .bytes;
        assert_eq!(tree.nodes[0].allocated, expected);
        let a = tree
            .children(0)
            .into_iter()
            .find(|id| tree.nodes[*id as usize].name.as_ref() == "a")
            .unwrap();
        let nested = tree.children(a)[0];
        let file = tree.children(nested)[0];
        assert_eq!(tree.path(file), fs::canonicalize(&original).unwrap());
        stop.store(true, Ordering::Relaxed);
        assert!(
            scan(&root, 4, &stop, &progress)
                .unwrap_err()
                .contains("cancelled")
        );
        fs::remove_dir_all(root).unwrap();
    }
}
