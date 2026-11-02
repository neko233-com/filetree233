use std::{fs, io, path::Path};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct FileId(pub u64, pub u64);
#[derive(Debug)]
pub struct Allocation {
    pub bytes: u64,
    pub shared_id: Option<FileId>,
}

#[cfg(unix)]
pub fn file_allocation(_: &Path, metadata: &fs::Metadata) -> io::Result<Allocation> {
    use std::os::unix::fs::MetadataExt;
    Ok(Allocation {
        bytes: metadata.blocks().saturating_mul(512),
        shared_id: (metadata.nlink() > 1).then_some(FileId(metadata.dev(), metadata.ino())),
    })
}

#[cfg(windows)]
pub fn file_allocation(path: &Path, metadata: &fs::Metadata) -> io::Result<Allocation> {
    use std::{
        mem::{size_of, zeroed},
        os::windows::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt},
            io::AsRawHandle,
        },
    };
    use windows_sys::Win32::{
        Foundation::{GetLastError, SetLastError},
        Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_COMPRESSED, FILE_ATTRIBUTE_SPARSE_FILE,
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
            FILE_SHARE_WRITE, FILE_STANDARD_INFO, FileStandardInfo, GetCompressedFileSizeW,
            GetFileInformationByHandle, GetFileInformationByHandleEx,
        },
    };
    let file = fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let handle = file.as_raw_handle() as _;
    let mut standard: FILE_STANDARD_INFO = unsafe { zeroed() };
    if unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileStandardInfo,
            &mut standard as *mut _ as _,
            size_of::<FILE_STANDARD_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let bytes = if metadata.file_attributes()
        & (FILE_ATTRIBUTE_COMPRESSED | FILE_ATTRIBUTE_SPARSE_FILE)
        != 0
    {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut high = 0u32;
        unsafe {
            SetLastError(0);
        }
        let low = unsafe { GetCompressedFileSizeW(wide.as_ptr(), &mut high) };
        if low == u32::MAX && unsafe { GetLastError() } != 0 {
            return Err(io::Error::last_os_error());
        }
        ((high as u64) << 32) | low as u64
    } else {
        u64::try_from(standard.AllocationSize)
            .map_err(|_| io::Error::other("negative allocation size"))?
    };
    let shared_id = if standard.NumberOfLinks > 1 {
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { zeroed() };
        if unsafe { GetFileInformationByHandle(handle, &mut info) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Some(FileId(
            info.dwVolumeSerialNumber as u64,
            ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64,
        ))
    } else {
        None
    };
    Ok(Allocation { bytes, shared_id })
}

#[cfg(not(any(unix, windows)))]
pub fn file_allocation(_: &Path, _: &fs::Metadata) -> io::Result<Allocation> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "allocation size is not supported on this platform",
    ))
}
