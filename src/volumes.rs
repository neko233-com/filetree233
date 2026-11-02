//! Cross-platform mounted-volume listing and explicit-path capacity queries.
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
#[derive(Debug, Clone)]
pub struct Volume {
    pub mount: PathBuf,
    pub name: String,
    pub filesystem: String,
    pub total: u64,
    pub available: u64,
    pub removable: bool,
    pub read_only: bool,
}
pub fn list() -> Result<Vec<Volume>, String> {
    let disks = sysinfo::Disks::new_with_refreshed_list().map_err(|e| e.to_string())?;
    let mut volumes: Vec<_> = disks
        .iter()
        .map(|disk| Volume {
            mount: disk.mount_point().to_path_buf(),
            name: disk.name().to_string_lossy().into_owned(),
            filesystem: disk.file_system().to_string_lossy().into_owned(),
            total: disk.total_space(),
            available: disk.available_space(),
            removable: disk.is_removable(),
            read_only: disk.is_read_only(),
        })
        .collect();
    volumes.sort_by(|a, b| a.mount.cmp(&b.mount));
    Ok(volumes)
}
pub fn list_json() -> Result<Value, String> {
    let volumes = list()?;
    let rows: Vec<_> = volumes.iter().map(|v| json!({"mount":v.mount.to_string_lossy(),"mount_units":crate::tree::raw_units(v.mount.as_os_str()),"name":v.name,"filesystem":v.filesystem,"total_bytes":v.total,"available_bytes":v.available,"capacity_known":v.total > 0,"removable":v.removable,"read_only":v.read_only})).collect();
    Ok(
        json!({"volumes":rows,"name_encoding":if cfg!(windows){"utf16"}else{"bytes"},"sampled_at_unix_seconds":SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e|e.to_string())?.as_secs(),"note":"Mounts and capacities are an OS snapshot; on Linux the automatic list excludes network and tmpfs mounts. Query an explicit path for its capacity."}),
    )
}
#[cfg(unix)]
pub fn capacity(path: &Path) -> Result<Value, String> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let root = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
    let cpath = CString::new(root.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(cpath.as_ptr(), &mut stats) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let block = if stats.f_frsize == 0 {
        stats.f_bsize as u64
    } else {
        stats.f_frsize as u64
    };
    if block == 0 {
        return Err("filesystem reported an invalid block size".into());
    }
    let total = (stats.f_blocks as u64)
        .checked_mul(block)
        .ok_or("capacity overflow")?;
    let free = (stats.f_bfree as u64)
        .checked_mul(block)
        .ok_or("capacity overflow")?;
    let available = (stats.f_bavail as u64)
        .checked_mul(block)
        .ok_or("capacity overflow")?;
    Ok(
        json!({"path":root.to_string_lossy(),"path_units":crate::tree::raw_units(root.as_os_str()),"name_encoding":"bytes","total_bytes":total,"free_bytes":free,"available_bytes":available,"note":"Filesystem capacity at query time; available space may be less than free space because of reserved blocks or quotas."}),
    )
}
#[cfg(windows)]
pub fn capacity(path: &Path) -> Result<Value, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let root = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
    let directory = if std::fs::metadata(&root)
        .map_err(|e| e.to_string())?
        .is_dir()
    {
        root.as_path()
    } else {
        root.parent().ok_or("path has no parent directory")?
    };
    let mut wide: Vec<u16> = directory.as_os_str().encode_wide().collect();
    if wide.last().copied() != Some(b'\\' as u16) {
        wide.push(b'\\' as u16);
    }
    wide.push(0);
    let (mut available, mut total, mut free) = (0u64, 0u64, 0u64);
    if unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut available, &mut total, &mut free) } == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(
        json!({"path":root.to_string_lossy(),"path_units":crate::tree::raw_units(root.as_os_str()),"name_encoding":"utf16","total_bytes":total,"free_bytes":free,"available_bytes":available,"note":"Filesystem capacity at query time. Available and total bytes can reflect the caller's disk quota."}),
    )
}
#[cfg(not(any(unix, windows)))]
pub fn capacity(_: &Path) -> Result<Value, String> {
    Err("capacity query is unsupported on this platform".into())
}
