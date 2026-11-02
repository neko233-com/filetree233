//! Content-verified duplicate search. Results never authorize deletion.
use crate::{allocation, is_reparse, tree::Snapshot};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::SystemTime,
};
const CHUNK: usize = 64 * 1024;
const MAX_ERRORS: usize = 50;
#[derive(Debug)]
pub struct Object {
    pub path: PathBuf,
    pub aliases: Vec<PathBuf>,
}
#[derive(Debug)]
pub struct Group {
    pub size: u64,
    pub sha256: String,
    pub objects: Vec<Object>,
}
#[derive(Debug)]
pub struct Report {
    pub groups: Vec<Group>,
    pub errors: Vec<String>,
    pub error_count: u64,
    pub complete: bool,
    pub candidate_objects: u64,
}
#[derive(Clone)]
struct Candidate {
    path: PathBuf,
    aliases: Vec<PathBuf>,
    size: u64,
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: SystemTime,
}
fn stamp(m: &fs::Metadata) -> io::Result<Stamp> {
    Ok(Stamp {
        len: m.len(),
        modified: m.modified()?,
    })
}
fn open_checked(path: &Path, expected: u64) -> Result<(File, Stamp), String> {
    let m = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !m.is_file() || m.file_type().is_symlink() || is_reparse(&m) {
        return Err("not a regular file".into());
    }
    let original = stamp(&m).map_err(|e| e.to_string())?;
    if original.len != expected {
        return Err("file size changed after scan".into());
    }
    let file = File::open(path).map_err(|e| e.to_string())?;
    if stamp(&file.metadata().map_err(|e| e.to_string())?).map_err(|e| e.to_string())? != original {
        return Err("file changed while opening".into());
    }
    Ok((file, original))
}
fn verify_stamp(path: &Path, file: &File, before: Stamp) -> Result<(), String> {
    let handle = stamp(&file.metadata().map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if metadata.file_type().is_symlink()
        || is_reparse(&metadata)
        || !metadata.is_file()
        || stamp(&metadata).map_err(|e| e.to_string())? != before
        || handle != before
    {
        return Err("file changed while reading".into());
    }
    Ok(())
}
fn digest(
    path: &Path,
    size: u64,
    sample: bool,
    cancelled: &AtomicBool,
) -> Result<[u8; 32], String> {
    let (mut file, before) = open_checked(path, size)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; CHUNK];
    if sample && size > (2 * CHUNK) as u64 {
        for offset in [0, size - CHUNK as u64] {
            if cancelled.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            file.seek(SeekFrom::Start(offset))
                .map_err(|e| e.to_string())?;
            file.read_exact(&mut buffer).map_err(|e| e.to_string())?;
            hasher.update(&buffer);
        }
    } else {
        loop {
            if cancelled.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
        }
    }
    verify_stamp(path, &file, before)?;
    Ok(hasher.finalize().into())
}
fn equal_content(a: &Path, b: &Path, size: u64, cancelled: &AtomicBool) -> Result<bool, String> {
    let (mut af, astamp) = open_checked(a, size)?;
    let (mut bf, bstamp) = open_checked(b, size)?;
    let mut ab = vec![0u8; CHUNK];
    let mut bb = vec![0u8; CHUNK];
    let mut equal = true;
    let mut remaining = size;
    while remaining > 0 {
        if cancelled.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let count = usize::try_from(remaining.min(CHUNK as u64)).expect("bounded chunk");
        af.read_exact(&mut ab[..count]).map_err(|e| e.to_string())?;
        bf.read_exact(&mut bb[..count]).map_err(|e| e.to_string())?;
        if ab[..count] != bb[..count] {
            equal = false;
            break;
        }
        remaining -= count as u64;
    }
    verify_stamp(a, &af, astamp)?;
    verify_stamp(b, &bf, bstamp)?;
    Ok(equal)
}
fn hash_jobs(
    candidates: &[Candidate],
    ids: &[usize],
    sample: bool,
    threads: u8,
    cancelled: &AtomicBool,
    progress: &AtomicU64,
) -> Vec<(usize, Result<[u8; 32], String>)> {
    let next = AtomicUsize::new(0);
    let results = Mutex::new(Vec::with_capacity(ids.len()));
    std::thread::scope(|scope| {
        for _ in 0..usize::min(threads as usize, ids.len()) {
            scope.spawn(|| {
                loop {
                    if cancelled.load(Ordering::Relaxed) {
                        break;
                    }
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&id) = ids.get(i) else { break };
                    let c = &candidates[id];
                    let result = digest(&c.path, c.size, sample, cancelled);
                    results
                        .lock()
                        .expect("hash results poisoned")
                        .push((id, result));
                    progress.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
    });
    results.into_inner().expect("hash results poisoned")
}
fn record_error(errors: &mut Vec<String>, count: &mut u64, path: &Path, message: &str) {
    *count += 1;
    if errors.len() < MAX_ERRORS {
        errors.push(format!("{}: {}", path.display(), message));
    }
}
pub fn find(
    snapshot: &Snapshot,
    threads: u8,
    cancelled: &AtomicBool,
    progress: &AtomicU64,
) -> Result<Report, String> {
    if !(1..=32).contains(&threads) {
        return Err("threads must be between 1 and 32".into());
    }
    let mut by_size: BTreeMap<u64, Vec<u32>> = BTreeMap::new();
    for (id, node) in snapshot.nodes.iter().enumerate() {
        if !node.is_dir && node.files == 1 && node.complete {
            by_size.entry(node.logical).or_default().push(id as u32);
        }
    }
    let mut candidates = Vec::<Candidate>::new();
    let mut errors = Vec::new();
    let mut error_count = 0u64;
    for (size, ids) in by_size.into_iter().filter(|(_, v)| v.len() > 1) {
        let mut objects: Vec<Candidate> = Vec::new();
        let mut identities: HashMap<allocation::FileId, usize> = HashMap::new();
        for id in ids {
            if cancelled.load(Ordering::Relaxed) {
                return Err("duplicate search cancelled".into());
            }
            let path = snapshot.path(id);
            let info = fs::symlink_metadata(&path).and_then(|m| {
                if !m.is_file() || m.file_type().is_symlink() || is_reparse(&m) || m.len() != size {
                    return Err(io::Error::other("file changed after scan"));
                }
                allocation::file_allocation(&path, &m)
            });
            match info {
                Ok(info) => {
                    if let Some(existing) =
                        info.shared_id.and_then(|key| identities.get(&key).copied())
                    {
                        objects[existing].aliases.push(path);
                    } else {
                        if let Some(key) = info.shared_id {
                            identities.insert(key, objects.len());
                        }
                        objects.push(Candidate {
                            path,
                            aliases: Vec::new(),
                            size,
                        });
                    }
                }
                Err(e) => record_error(&mut errors, &mut error_count, &path, &e.to_string()),
            }
        }
        if objects.len() > 1 {
            candidates.extend(objects);
        }
    }
    let ids: Vec<usize> = (0..candidates.len()).collect();
    let mut samples: HashMap<(u64, [u8; 32]), Vec<usize>> = HashMap::new();
    for (id, result) in hash_jobs(&candidates, &ids, true, threads, cancelled, progress) {
        match result {
            Ok(hash) => samples
                .entry((candidates[id].size, hash))
                .or_default()
                .push(id),
            Err(e) => record_error(&mut errors, &mut error_count, &candidates[id].path, &e),
        }
    }
    if cancelled.load(Ordering::Relaxed) {
        return Err("duplicate search cancelled".into());
    }
    let mut full_groups: HashMap<(u64, [u8; 32]), Vec<usize>> = HashMap::new();
    let mut full_ids = Vec::new();
    for ((size, hash), group) in samples.into_iter().filter(|(_, v)| v.len() > 1) {
        if size <= (2 * CHUNK) as u64 {
            full_groups.insert((size, hash), group);
        } else {
            full_ids.extend(group);
        }
    }
    for (id, result) in hash_jobs(&candidates, &full_ids, false, threads, cancelled, progress) {
        match result {
            Ok(hash) => full_groups
                .entry((candidates[id].size, hash))
                .or_default()
                .push(id),
            Err(e) => record_error(&mut errors, &mut error_count, &candidates[id].path, &e),
        }
    }
    let mut groups = Vec::new();
    for ((size, hash), ids) in full_groups.into_iter().filter(|(_, v)| v.len() > 1) {
        let mut clusters: Vec<Vec<usize>> = Vec::new();
        let mut bucket_failed = false;
        for id in ids {
            let mut matched = false;
            let mut failed = false;
            for cluster in &mut clusters {
                match equal_content(
                    &candidates[cluster[0]].path,
                    &candidates[id].path,
                    size,
                    cancelled,
                ) {
                    Ok(true) => {
                        cluster.push(id);
                        matched = true;
                        break;
                    }
                    Ok(false) => {}
                    Err(e) => {
                        record_error(&mut errors, &mut error_count, &candidates[id].path, &e);
                        failed = true;
                        bucket_failed = true;
                        break;
                    }
                }
            }
            if failed {
                break;
            }
            if !matched {
                clusters.push(vec![id]);
            }
        }
        if bucket_failed {
            continue;
        }
        for cluster in clusters.into_iter().filter(|v| v.len() > 1) {
            let mut objects: Vec<_> = cluster
                .into_iter()
                .map(|id| Object {
                    path: candidates[id].path.clone(),
                    aliases: candidates[id].aliases.clone(),
                })
                .collect();
            objects.sort_by(|a, b| a.path.cmp(&b.path));
            groups.push(Group {
                size,
                sha256: hash.iter().map(|byte| format!("{byte:02x}")).collect(),
                objects,
            });
        }
    }
    if cancelled.load(Ordering::Relaxed) {
        return Err("duplicate search cancelled".into());
    }
    groups.sort_by(|a, b| b.size.cmp(&a.size).then(a.sha256.cmp(&b.sha256)));
    Ok(Report {
        groups,
        errors,
        error_count,
        complete: snapshot.nodes[0].complete && error_count == 0,
        candidate_objects: candidates.len() as u64,
    })
}
pub fn find_path(path: &Path, threads: u8) -> Result<Report, String> {
    let (cancelled, progress) = crate::tree::new_controls();
    let snapshot = crate::tree::scan(path, threads, &cancelled, &progress)?;
    progress.store(0, Ordering::Relaxed);
    find(&snapshot, threads, &cancelled, &progress)
}
impl Report {
    pub fn json(&self) -> serde_json::Value {
        let groups:Vec<_>=self.groups.iter().map(|g|serde_json::json!({"size":g.size,"sha256":g.sha256,"objects":g.objects.iter().map(|o|serde_json::json!({"path":o.path.to_string_lossy(),"path_units":crate::tree::raw_units(o.path.as_os_str()),"aliases":o.aliases.iter().map(|p|serde_json::json!({"path":p.to_string_lossy(),"path_units":crate::tree::raw_units(p.as_os_str())})).collect::<Vec<_>>()})).collect::<Vec<_>>() })).collect();
        serde_json::json!({"groups":groups,"candidate_objects":self.candidate_objects,"complete":self.complete,"error_count":self.error_count,"errors":self.errors,"verification":"sha256_and_byte_comparison","name_encoding":if cfg!(windows){"utf16"}else{"bytes"},"note":"Hard links are aliases, not independent copies. Actual reclaimable space may differ; results are a snapshot."})
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn demo_finds_content_duplicates_but_not_hardlink_aliases_or_sample_collisions() {
        let root = std::env::temp_dir().join(format!(
            "filetree233-duplicates-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("a")).unwrap();
        fs::create_dir_all(root.join("b")).unwrap();
        let mut content = vec![11u8; 200_000];
        content[90_000] = 42;
        let original = root.join("a/first.bin");
        fs::write(&original, &content).unwrap();
        fs::write(root.join("b/different-name.bin"), &content).unwrap();
        fs::hard_link(&original, root.join("a/alias.bin")).unwrap();
        content[90_000] = 43;
        fs::write(root.join("a/same-sample-but-different.bin"), &content).unwrap();
        fs::write(root.join("a/same-name.txt"), b"abc").unwrap();
        fs::write(root.join("b/same-name.txt"), b"xyz").unwrap();
        fs::write(root.join("a/empty"), b"").unwrap();
        fs::write(root.join("b/empty"), b"").unwrap();
        let (cancelled, progress) = crate::tree::new_controls();
        let snapshot = crate::tree::scan(&root, 4, &cancelled, &progress).unwrap();
        let result = find(&snapshot, 4, &cancelled, &progress).unwrap();
        assert!(result.complete);
        assert_eq!(result.groups.len(), 2);
        let big = result.groups.iter().find(|g| g.size == 200_000).unwrap();
        assert_eq!(big.objects.len(), 2);
        assert_eq!(
            big.objects.iter().map(|o| o.aliases.len()).sum::<usize>(),
            1
        );
        assert!(
            big.objects
                .iter()
                .all(|o| !o.path.ends_with("same-sample-but-different.bin"))
        );
        assert_eq!(
            result
                .groups
                .iter()
                .find(|g| g.size == 0)
                .unwrap()
                .objects
                .len(),
            2
        );
        fs::write(&original, b"changed").unwrap();
        let changed = find(&snapshot, 2, &cancelled, &progress).unwrap();
        assert!(!changed.complete);
        assert!(changed.error_count > 0);
        cancelled.store(true, Ordering::Relaxed);
        assert!(find(&snapshot, 1, &cancelled, &progress).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
