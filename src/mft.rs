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
pub fn inspect_volume(path: &Path, name: Option<&str>, out: &mut impl Write) -> Result<(), String> {
    let volume = ntfs_mft::Volume::from_mount_point(path).map_err(|e| e.to_string())?;
    let raw = volume.raw_mft().map_err(|e| e.to_string())?;
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
                let links: Vec<_> = entry.links.iter().map(|link| json!({"parent_reference":link.parent_reference.get(),"namespace":format!("{:?}",link.namespace),"name":link.file_name.to_string_lossy(),"name_utf16":link.file_name.as_utf16()})).collect();
                let ads: Vec<_> = entry.alternate_data_streams.iter().map(|stream| json!({"name":stream.name.to_string_lossy(),"name_utf16":stream.name.as_utf16(),"logical_bytes":stream.real_size,"allocated_bytes":stream.allocated_size,"resident":stream.is_resident})).collect();
                let value = json!({"type":"record","record_number":entry.record_number,"file_reference":entry.file_reference.get(),"parent_reference":entry.parent_reference.get(),"base_reference":entry.base_reference().map(|v| v.get()),"name":entry.file_name.to_string_lossy(),"name_utf16":entry.file_name.as_utf16(),"in_use":entry.is_used,"directory":entry.is_directory,"reparse":entry.is_reparse_point,"hard_links":entry.hard_link_count,"logical_bytes":entry.real_size,"allocated_bytes":entry.allocated_size,"links":links,"alternate_streams":ads,"metadata":format!("{:?}",record.metadata),"diagnostics":format!("{:?}",record.diagnostics),"external_payload_count":record.external_payloads.len()});
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
pub fn inspect_volume_stdout(path: &Path, name: Option<&str>) -> Result<(), String> {
    inspect_volume(path, name, &mut io::stdout().lock())
}
pub fn inspect_stdout(path: &Path, record_size: usize, used_only: bool) -> Result<(), String> {
    inspect(path, record_size, used_only, &mut io::stdout().lock())
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
