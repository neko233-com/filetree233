//! Native identity information for cleanup-plan fingerprints.
use sha2::{Digest, Sha256};
use std::{ffi::OsStr, fs, io, path::Path};

pub fn hash_name(hasher: &mut Sha256, name: &OsStr) {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let bytes = name.as_bytes();
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let units: Vec<u16> = name.encode_wide().collect();
        hasher.update((units.len() as u64).to_le_bytes());
        for unit in units {
            hasher.update(unit.to_le_bytes());
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let encoded = name.as_encoded_bytes();
        hasher.update((encoded.len() as u64).to_le_bytes());
        hasher.update(encoded);
    }
}
#[cfg(unix)]
pub fn hash_identity(hasher: &mut Sha256, _: &Path, metadata: &fs::Metadata) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    hasher.update(metadata.dev().to_le_bytes());
    hasher.update(metadata.ino().to_le_bytes());
    hasher.update(metadata.ctime().to_le_bytes());
    hasher.update(metadata.ctime_nsec().to_le_bytes());
    Ok(())
}
#[cfg(windows)]
pub fn hash_identity(hasher: &mut Sha256, path: &Path, _: &fs::Metadata) -> io::Result<()> {
    use std::{
        mem::{size_of, zeroed},
        os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    };
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT, FILE_BASIC_INFO,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileBasicInfo,
        GetFileInformationByHandle, GetFileInformationByHandleEx,
    };
    let file = fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let handle = file.as_raw_handle() as _;
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { zeroed() };
    if unsafe { GetFileInformationByHandle(handle, &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::other("file became a reparse point"));
    }
    let mut basic: FILE_BASIC_INFO = unsafe { zeroed() };
    if unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileBasicInfo,
            &mut basic as *mut _ as _,
            size_of::<FILE_BASIC_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    hasher.update(info.dwVolumeSerialNumber.to_le_bytes());
    hasher.update(info.nFileIndexHigh.to_le_bytes());
    hasher.update(info.nFileIndexLow.to_le_bytes());
    hasher.update(basic.CreationTime.to_le_bytes());
    hasher.update(basic.ChangeTime.to_le_bytes());
    Ok(())
}
#[cfg(not(any(unix, windows)))]
pub fn hash_identity(_: &mut Sha256, _: &Path, _: &fs::Metadata) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "cleanup plan identity is unsupported on this platform",
    ))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn distinct_raw_names_do_not_collapse_to_the_same_fingerprint() {
        #[cfg(unix)]
        let (a, b) = {
            use std::os::unix::ffi::OsStringExt;
            (
                std::ffi::OsString::from_vec(vec![0x80]),
                std::ffi::OsString::from_vec(vec![0x81]),
            )
        };
        #[cfg(windows)]
        let (a, b) = {
            use std::os::windows::ffi::OsStringExt;
            (
                std::ffi::OsString::from_wide(&[0xd800]),
                std::ffi::OsString::from_wide(&[0xd801]),
            )
        };
        assert_eq!(a.to_string_lossy(), b.to_string_lossy());
        let mut first = Sha256::new();
        let mut second = Sha256::new();
        hash_name(&mut first, &a);
        hash_name(&mut second, &b);
        assert_ne!(first.finalize(), second.finalize());
    }
}
