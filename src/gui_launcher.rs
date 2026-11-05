//! Windowless GUI entry point.
//!
//! A console-subsystem executable gets a console window from Explorer before any
//! Rust code can run, so hiding it at startup always leaves a flash. Shortcuts
//! start this GUI-subsystem launcher instead; it starts `filetree233 gui` with
//! `CREATE_NO_WINDOW`, and the console binary keeps its normal stdout behaviour
//! when people run it from a terminal.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    if let Err(message) = launch() {
        show_error(&message);
    }
}

#[cfg(windows)]
fn launch() -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    // The GUI child must never own a console, including on systems where the
    // default terminal is Windows Terminal.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let launcher =
        std::env::current_exe().map_err(|e| format!("cannot locate the launcher: {e}"))?;
    let exe = launcher.with_file_name("filetree233.exe");
    std::process::Command::new(&exe)
        .arg("gui")
        .args(std::env::args_os().skip(1))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("cannot start {}: {e}", exe.display()))
}

#[cfg(windows)]
fn show_error(message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    let text: Vec<u16> = format!("filetree233 could not open.\n\n{message}")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let title: Vec<u16> = "filetree233"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[cfg(not(windows))]
fn main() {
    if let Err(message) = launch() {
        eprintln!("filetree233: {message}");
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn launch() -> Result<(), String> {
    let launcher =
        std::env::current_exe().map_err(|e| format!("cannot locate the launcher: {e}"))?;
    let exe = launcher.with_file_name("filetree233");
    let status = std::process::Command::new(&exe)
        .arg("gui")
        .args(std::env::args_os().skip(1))
        .status()
        .map_err(|e| format!("cannot start {}: {e}", exe.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{} exited with {status}", exe.display()))
    }
}
