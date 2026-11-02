//! System trash operations. Never fall back to permanent deletion.
use std::path::Path;

#[cfg(not(windows))]
pub fn send(path: &Path) -> Result<(), String> {
    trash::delete(path).map_err(|error| error.to_string())
}

#[cfg(windows)]
pub fn send(path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{
        Win32::{
            System::Com::{
                CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize,
            },
            UI::Shell::{
                FOF_NOERRORUI, FOF_SILENT, FOF_WANTNUKEWARNING, FOFX_EARLYFAILURE,
                FOFX_RECYCLEONDELETE, FileOperation, IFileOperation, IShellItem,
                SHCreateItemFromParsingName,
            },
        },
        core::PCWSTR,
    };
    let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    // The shell cannot parse verbatim drive paths. Reject network and device paths:
    // their recycle-bin behavior is not equivalent to a local volume.
    let prefix: Vec<u16> = r"\\?\".encode_utf16().collect();
    if !wide.starts_with(&prefix)
        || wide.len() < 7
        || wide[5] != b':' as u16
        || wide[6] != b'\\' as u16
        || !(wide[4] as u8).is_ascii_alphabetic()
    {
        return Err("recycle bin requires a local drive path".into());
    }
    let mut shell_path = wide[4..].to_vec();
    shell_path.push(0);
    std::thread::spawn(move || unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|e| e.to_string())?;
        struct Apartment;
        impl Drop for Apartment {
            fn drop(&mut self) {
                unsafe {
                    CoUninitialize();
                }
            }
        }
        let _apartment = Apartment;
        let operation: IFileOperation =
            CoCreateInstance(&FileOperation, None, CLSCTX_ALL).map_err(|e| e.to_string())?;
        operation
            .SetOperationFlags(
                FOFX_RECYCLEONDELETE
                    | FOFX_EARLYFAILURE
                    | FOF_NOERRORUI
                    | FOF_SILENT
                    | FOF_WANTNUKEWARNING,
            )
            .map_err(|e| e.to_string())?;
        let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(shell_path.as_ptr()), None)
            .map_err(|e| e.to_string())?;
        operation
            .DeleteItem(&item, None)
            .map_err(|e| e.to_string())?;
        operation.PerformOperations().map_err(|e| e.to_string())?;
        if operation
            .GetAnyOperationsAborted()
            .map_err(|e| e.to_string())?
            .as_bool()
        {
            return Err("recycle operation aborted".into());
        }
        Ok(())
    })
    .join()
    .map_err(|_| "recycle worker failed".to_string())?
}
