use ntfs_mft::{MftStream, MftStreamOptions};
use serde_json::json;
use std::{
    io::{self, Read, Seek, Write},
    path::Path,
};

/// Inspect a logical MFT stream without retaining its full contents in memory.
/// Output is JSON Lines, including parse errors and explicit final counts.
pub fn inspect(
    path: &Path,
    record_size: usize,
    used_only: bool,
    out: &mut impl Write,
) -> Result<(), String> {
    let options = MftStreamOptions::new(record_size)
        .map_err(|e| e.to_string())?
        .resolve_extensions(true);
    let mut stream = MftStream::from_path(path, options).map_err(|e| e.to_string())?;
    inspect_stream(
        &mut stream,
        used_only,
        None,
        false,
        "logical_mft_stream",
        out,
    )
}

/// Read directly from an NTFS volume. This is observational and does not authorize cleanup.
pub fn inspect_volume(
    path: &Path,
    name: Option<&str>,
    fast: bool,
    out: &mut impl Write,
) -> Result<(), String> {
    let volume = ntfs_mft::Volume::from_mount_point(path).map_err(|e| e.to_string())?;
    let raw = volume.raw_mft().map_err(|e| e.to_string())?;
    if fast {
        let mut parsed = 0u64;
        let mut displayed = 0u64;
        for result in raw.scan_entries().map_err(|e| e.to_string())? {
            let entry = result.map_err(|e| e.to_string())?;
            parsed += 1;
            if name.is_some_and(|needle| !entry.file_name.to_string_lossy().contains(needle)) {
                continue;
            }
            let metadata = format!("{:?}", entry.metadata());
            let value = entry_value(&entry, &metadata, None, None);
            writeln!(out, "{value}").map_err(|e| e.to_string())?;
            displayed += 1;
        }
        writeln!(out, "{}", json!({"type":"summary","parsed_records":parsed,"invalid_records":null,"displayed_records":displayed,"source":"live_ntfs_volume_fast","complete":false,"note":"Fast MFT scan may silently skip malformed records or incomplete attributes; counts and metadata are best effort"})).map_err(|e| e.to_string())?;
        return out.flush().map_err(|e| e.to_string());
    }
    let options = MftStreamOptions::new(raw.file_record_size())
        .map_err(|e| e.to_string())?
        .resolve_extensions(true);
    // Batch sequential records while keeping the read buffer bounded.
    let reader = std::io::BufReader::with_capacity(
        1024 * 1024,
        raw.byte_reader().map_err(|e| e.to_string())?,
    );
    let mut stream = MftStream::from_reader(reader, options).map_err(|e| e.to_string())?;
    inspect_stream(
        &mut stream,
        true,
        name,
        name.is_some(),
        "live_ntfs_volume",
        out,
    )
}

fn inspect_stream<R: Read + Seek>(
    stream: &mut MftStream<R>,
    used_only: bool,
    name: Option<&str>,
    hide_invalid: bool,
    source: &str,
    out: &mut impl Write,
) -> Result<(), String> {
    let mut parsed = 0u64;
    let mut invalid = 0u64;
    let mut displayed = 0u64;
    for (index, record) in stream.iter_records().enumerate() {
        match record {
            Ok(record) => {
                parsed += 1;
                if used_only && !record.entry.is_used {
                    continue;
                }
                let entry = record.entry;
                if name.is_some_and(|needle| !entry.file_name.to_string_lossy().contains(needle)) {
                    continue;
                }
                let value = entry_value(
                    &entry,
                    &format!("{:?}", record.metadata),
                    Some(&format!("{:?}", record.diagnostics)),
                    Some(record.external_payloads.len()),
                );
                writeln!(out, "{value}").map_err(|e| e.to_string())?;
                displayed += 1;
            }
            Err(e) => {
                if e.is_io_error() {
                    return Err(format!("MFT read stopped at record {index}: {e}"));
                }
                invalid += 1;
                if hide_invalid {
                    continue;
                }
                writeln!(
                    out,
                    "{}",
                    json!({"type":"invalid_record","record_number":index,"error":e.to_string()})
                )
                .map_err(|e| e.to_string())?;
            }
        }
    }
    writeln!(out, "{}", json!({"type":"summary","parsed_records":parsed,"invalid_records":invalid,"displayed_records":displayed,"source":source,"complete":false,"note":"MFT-only data cannot prove complete metadata or live filesystem state"})).map_err(|e| e.to_string())?;
    out.flush().map_err(|e| e.to_string())
}
fn entry_value(
    entry: &ntfs_mft::RawMftEntry,
    metadata: &str,
    diagnostics: Option<&str>,
    external_payload_count: Option<usize>,
) -> serde_json::Value {
    let links: Vec<_> = entry.links.iter().map(|link| json!({"parent_reference":link.parent_reference.get(),"namespace":format!("{:?}",link.namespace),"name":link.file_name.to_string_lossy(),"name_utf16":link.file_name.as_utf16()})).collect();
    let ads: Vec<_> = entry.alternate_data_streams.iter().map(|stream| json!({"name":stream.name.to_string_lossy(),"name_utf16":stream.name.as_utf16(),"logical_bytes":stream.real_size,"allocated_bytes":stream.allocated_size,"resident":stream.is_resident})).collect();
    json!({"type":"record","record_number":entry.record_number,"file_reference":entry.file_reference.get(),"parent_reference":entry.parent_reference.get(),"base_reference":entry.base_reference().map(|v| v.get()),"name":entry.file_name.to_string_lossy(),"name_utf16":entry.file_name.as_utf16(),"in_use":entry.is_used,"directory":entry.is_directory,"reparse":entry.is_reparse_point,"hard_links":entry.hard_link_count,"logical_bytes":entry.real_size,"allocated_bytes":entry.allocated_size,"links":links,"alternate_streams":ads,"metadata":metadata,"diagnostics":diagnostics,"external_payload_count":external_payload_count})
}
pub fn inspect_volume_stdout(path: &Path, name: Option<&str>, fast: bool) -> Result<(), String> {
    inspect_volume(path, name, fast, &mut io::stdout().lock())
}
pub fn inspect_stdout(path: &Path, record_size: usize, used_only: bool) -> Result<(), String> {
    inspect(path, record_size, used_only, &mut io::stdout().lock())
}
#[cfg(windows)]
pub fn scan_tree(path: &Path) -> Result<serde_json::Value, String> {
    let (cancelled, progress) = crate::tree::new_controls();
    let (index, root) = scan_index(path, false, &cancelled, &progress)?;
    Ok(index.finish(&root))
}
#[cfg(windows)]
pub fn scan_snapshot(
    path: &Path,
    cancelled: &std::sync::atomic::AtomicBool,
    progress: &std::sync::atomic::AtomicU64,
) -> Result<crate::tree::Snapshot, String> {
    let (index, root) = scan_index(path, true, cancelled, progress)?;
    Ok(index.finish_snapshot(&root))
}
#[cfg(not(windows))]
pub fn scan_snapshot(
    _: &Path,
    _: &std::sync::atomic::AtomicBool,
    _: &std::sync::atomic::AtomicU64,
) -> Result<crate::tree::Snapshot, String> {
    Err("MFT tree scanning currently supports Windows only".into())
}
#[cfg(windows)]
fn scan_index(
    path: &Path,
    snapshot: bool,
    cancelled: &std::sync::atomic::AtomicBool,
    progress: &std::sync::atomic::AtomicU64,
) -> Result<(crate::mft_index::Index, std::path::PathBuf), String> {
    use crate::mft_index::{Directory, Index, Link};
    use ntfs_mft::FileNameNamespace;
    use std::sync::atomic::Ordering;
    use std::{
        ffi::OsString,
        fs,
        mem::zeroed,
        os::windows::{
            ffi::{OsStrExt, OsStringExt},
            fs::OpenOptionsExt,
            io::AsRawHandle,
        },
    };
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        GetFileInformationByHandle, GetVolumePathNameW,
    };
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_dir() || crate::is_reparse(&meta) {
        return Err("MFT scan requires a non-reparse directory".into());
    }
    let root = fs::canonicalize(path).map_err(|e| e.to_string())?;
    let dir = fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&root)
        .map_err(|e| e.to_string())?;
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { zeroed() };
    if unsafe { GetFileInformationByHandle(dir.as_raw_handle() as _, &mut info) } == 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    let root_id = ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64;
    let wide: Vec<u16> = root.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut mount = vec![0u16; 32768];
    if unsafe { GetVolumePathNameW(wide.as_ptr(), mount.as_mut_ptr(), mount.len() as u32) } == 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    let n = mount
        .iter()
        .position(|c| *c == 0)
        .ok_or("invalid mount path")?;
    let mount = OsString::from_wide(&mount[..n]);
    // ntfs-mft currently converts mount-point paths to UTF-8 internally. Reject
    // an unrepresentable mount rather than open a different, lossy path.
    let mount_text = mount.to_str().ok_or(
        "non-Unicode volume mount is unsupported by the MFT backend; use filesystem traversal",
    )?;
    let display_mount = mount_text.strip_prefix(r"\\?\").unwrap_or(mount_text);
    let b = display_mount.as_bytes();
    let volume = if b.len() == 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\' {
        ntfs_mft::Volume::from_drive_letter(b[0] as char).map_err(|e| e.to_string())?
    } else {
        // The dependency adds its own trailing backslash for a mounted folder.
        ntfs_mft::Volume::from_mount_point(Path::new(display_mount.trim_end_matches('\\')))
            .map_err(|e| e.to_string())?
    };
    let raw = volume.raw_mft().map_err(|e| e.to_string())?;
    let mut index = Index::new(root_id);
    if snapshot {
        index.enable_snapshot();
    }
    let mut root_found = root_id & 0x0000_ffff_ffff_ffff == 5;
    for record in raw.scan_entries().map_err(|e| e.to_string())? {
        if cancelled.load(Ordering::Relaxed) {
            return Err("scan cancelled".into());
        }
        progress.fetch_add(1, Ordering::Relaxed);
        let entry = record.map_err(|e| e.to_string())?;
        if entry.file_reference.get() == root_id {
            root_found = true;
        }
        if entry.is_directory && entry.file_reference.get() != root_id {
            index.add_directory(Directory {
                id: entry.file_reference.get(),
                parent: entry.parent_reference.get(),
                name: OsString::from_wide(entry.file_name.as_utf16()),
                reparse: entry.is_reparse_point,
            });
        }
    }
    if !root_found {
        return Err("target directory reference not found in MFT; use filesystem scan".into());
    }
    index.finish_directories();
    // Most records of a volume are outside a subdirectory scan. Avoid creating
    // link names and opening resident/sparse files until a parent is in scope.
    // For a volume-root scan, avoid the extra lookup on every file.
    let filter_parents = root_id & 0x0000_ffff_ffff_ffff != 5;
    for record in raw.scan_entries().map_err(|e| e.to_string())? {
        if cancelled.load(Ordering::Relaxed) {
            return Err("scan cancelled".into());
        }
        progress.fetch_add(1, Ordering::Relaxed);
        let entry = record.map_err(|e| e.to_string())?;
        if entry.is_directory || entry.is_reparse_point {
            continue;
        }
        let mut links = Vec::new();
        if entry.links.is_empty() {
            if filter_parents && !index.includes_file_parent(entry.parent_reference.get()) {
                continue;
            }
            links.push(Link {
                parent: entry.parent_reference.get(),
                name: OsString::from_wide(entry.file_name.as_utf16()),
            });
        } else {
            for link in &entry.links {
                if link.namespace == FileNameNamespace::Dos
                    && entry.links.iter().any(|other| {
                        other.parent_reference == link.parent_reference
                            && other.namespace != FileNameNamespace::Dos
                    })
                {
                    continue;
                }
                if filter_parents && !index.includes_file_parent(link.parent_reference.get()) {
                    continue;
                }
                links.push(Link {
                    parent: link.parent_reference.get(),
                    name: OsString::from_wide(link.file_name.as_utf16()),
                });
            }
        }
        if links.is_empty() {
            continue;
        }
        // Only materialize a full path when special allocation needs a Win32 query.
        let allocated = if entry.is_resident || entry.is_sparse || entry.is_compressed {
            let Some(path) = index.file_path(&root, &links) else {
                continue;
            };
            match fs::symlink_metadata(&path).and_then(|m| {
                if crate::is_reparse(&m) || !m.is_file() {
                    return Err(io::Error::other("file changed during scan"));
                }
                crate::allocation::file_allocation(&path, &m)
            }) {
                Ok(a) => a.bytes,
                Err(_) => {
                    index.note_allocation_unknown();
                    0
                }
            }
        } else {
            entry.allocated_size
        };
        index.add_file(&links, entry.real_size, allocated);
    }
    Ok((index, root))
}
#[cfg(not(windows))]
pub fn scan_tree(_: &Path) -> Result<serde_json::Value, String> {
    Err("MFT directory aggregation currently supports Windows only".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        sync::atomic::{AtomicUsize, Ordering},
    };
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    fn synthetic_record(used: bool) -> [u8; 1024] {
        let mut record = [0u8; 1024];
        record[0..4].copy_from_slice(b"FILE");
        record[4..6].copy_from_slice(&48u16.to_le_bytes());
        record[6..8].copy_from_slice(&3u16.to_le_bytes());
        record[16..18].copy_from_slice(&1u16.to_le_bytes());
        record[18..20].copy_from_slice(&1u16.to_le_bytes());
        record[20..22].copy_from_slice(&56u16.to_le_bytes());
        record[22..24].copy_from_slice(&(used as u16).to_le_bytes());
        record[24..28].copy_from_slice(&64u32.to_le_bytes());
        record[28..32].copy_from_slice(&1024u32.to_le_bytes());
        record[48..50].copy_from_slice(&1u16.to_le_bytes());
        record[510..512].copy_from_slice(&1u16.to_le_bytes());
        record[1022..1024].copy_from_slice(&1u16.to_le_bytes());
        record[56..60].copy_from_slice(&u32::MAX.to_le_bytes());
        record
    }
    fn add_resident(
        record: &mut [u8],
        offset: usize,
        kind: u32,
        name: &str,
        content: &[u8],
    ) -> usize {
        let units: Vec<u16> = name.encode_utf16().collect();
        let data_offset = (24 + units.len() * 2 + 7) & !7;
        let length = (data_offset + content.len() + 7) & !7;
        record[offset..offset + 4].copy_from_slice(&kind.to_le_bytes());
        record[offset + 4..offset + 8].copy_from_slice(&(length as u32).to_le_bytes());
        record[offset + 9] = units.len() as u8;
        record[offset + 10..offset + 12].copy_from_slice(&24u16.to_le_bytes());
        record[offset + 16..offset + 20].copy_from_slice(&(content.len() as u32).to_le_bytes());
        record[offset + 20..offset + 22].copy_from_slice(&(data_offset as u16).to_le_bytes());
        for (i, unit) in units.iter().enumerate() {
            record[offset + 24 + i * 2..offset + 26 + i * 2].copy_from_slice(&unit.to_le_bytes());
        }
        record[offset + data_offset..offset + data_offset + content.len()].copy_from_slice(content);
        offset + length
    }
    fn named_record() -> [u8; 1024] {
        let mut record = synthetic_record(true);
        let units: Vec<u16> = "demo-雪.txt".encode_utf16().collect();
        let mut name = vec![0u8; 66 + units.len() * 2];
        name[..8].copy_from_slice(&((1u64 << 48) | 5).to_le_bytes());
        name[64] = units.len() as u8;
        name[65] = 1;
        for (i, unit) in units.iter().enumerate() {
            name[66 + i * 2..68 + i * 2].copy_from_slice(&unit.to_le_bytes());
        }
        let mut offset = add_resident(&mut record, 56, 0x30, "", &name);
        offset = add_resident(&mut record, offset, 0x80, "", b"hello");
        offset = add_resident(&mut record, offset, 0x80, "extra", b"ads");
        record[offset..offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        record[24..28].copy_from_slice(&((offset + 8) as u32).to_le_bytes());
        record
    }
    #[test]
    fn live_stream_filter_does_not_emit_unmatched_or_invalid_records() {
        let mut bytes = Vec::new();
        bytes.extend(named_record());
        bytes.extend(synthetic_record(true));
        bytes.extend([0u8; 1024]);
        let mut stream = MftStream::from_reader(
            std::io::Cursor::new(bytes),
            MftStreamOptions::new(1024).unwrap(),
        )
        .unwrap();
        let mut out = Vec::new();
        inspect_stream(&mut stream, true, Some("雪"), true, "test", &mut out).unwrap();
        let rows: Vec<serde_json::Value> = out
            .split(|b| *b == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice(line).unwrap())
            .collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["name"], "demo-雪.txt");
        assert_eq!(rows[1]["parsed_records"], 2);
        assert_eq!(rows[1]["invalid_records"], 1);
        assert_eq!(rows[1]["displayed_records"], 1);
    }
    #[test]
    fn stream_io_error_stops_scan_without_success_summary() {
        struct Failing(std::io::Cursor<Vec<u8>>);
        impl std::io::Read for Failing {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if self.0.position() >= 1024 {
                    return Err(std::io::Error::other("injected read failure"));
                }
                self.0.read(buf)
            }
        }
        impl std::io::Seek for Failing {
            fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
                self.0.seek(pos)
            }
        }
        let mut bytes = Vec::new();
        bytes.extend(named_record());
        bytes.extend(synthetic_record(true));
        let mut stream = MftStream::from_reader(
            Failing(std::io::Cursor::new(bytes)),
            MftStreamOptions::new(1024).unwrap(),
        )
        .unwrap();
        let mut out = Vec::new();
        let error = inspect_stream(&mut stream, false, None, false, "test", &mut out).unwrap_err();
        assert!(error.contains("injected read failure"));
        assert!(!String::from_utf8(out).unwrap().contains("summary"));
    }
    #[test]
    fn exported_mft_preserves_name_size_and_named_streams() {
        let path = std::env::temp_dir().join(format!(
            "filetree233-mft-named-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&path, named_record()).unwrap();
        let mut out = Vec::new();
        inspect(&path, 1024, false, &mut out).unwrap();
        let row: serde_json::Value =
            serde_json::from_slice(out.split(|b| *b == b'\n').next().unwrap()).unwrap();
        assert_eq!(row["name"], "demo-雪.txt");
        assert_eq!(row["logical_bytes"], 5);
        assert_eq!(row["alternate_streams"][0]["name"], "extra");
        assert_eq!(row["alternate_streams"][0]["logical_bytes"], 3);
        fs::remove_file(path).unwrap();
    }
    #[test]
    fn exported_mft_reports_valid_and_invalid_slots_without_claiming_completeness() {
        let path = std::env::temp_dir().join(format!(
            "filetree233-mft-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut bytes = Vec::new();
        bytes.extend(synthetic_record(true));
        bytes.extend(synthetic_record(false));
        bytes.extend([0u8; 1024]);
        fs::write(&path, bytes).unwrap();
        let mut out = Vec::new();
        inspect(&path, 1024, true, &mut out).unwrap();
        let rows: Vec<serde_json::Value> = out
            .split(|b| *b == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice(line).unwrap())
            .collect();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["type"], "record");
        assert_eq!(rows[0]["record_number"], 0);
        assert_eq!(rows[1]["type"], "invalid_record");
        assert_eq!(rows[2]["parsed_records"], 2);
        assert_eq!(rows[2]["invalid_records"], 1);
        assert_eq!(rows[2]["displayed_records"], 1);
        assert_eq!(rows[2]["complete"], false);
        assert!(inspect(&path, 1000, false, &mut Vec::new()).is_err());
        fs::remove_file(path).unwrap();
    }
}
