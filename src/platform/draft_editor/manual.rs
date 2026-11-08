use std::{io, path::Path, process::Command};

#[cfg(not(windows))]
pub(super) fn manual_editor(file: &Path, editor: &str) -> io::Result<()> {
    if super::wrapper(editor) {
        return Err(io::Error::other("Recursive Bus editor configuration"));
    }
    let status = Command::new("/bin/sh")
        .args(["-c", &format!("exec {editor} \"$@\""), "bus-draft-editor"])
        .arg(file)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other("The developer's editor failed"))
    }
}

#[cfg(windows)]
pub(super) fn manual_editor(file: &Path, editor: &str) -> io::Result<()> {
    use windows_sys::Win32::{Foundation::LocalFree, UI::Shell::CommandLineToArgvW};
    if super::wrapper(editor) {
        return Err(io::Error::other("Recursive Bus editor configuration"));
    }
    let command: Vec<_> = editor.encode_utf16().chain(Some(0)).collect();
    let mut count = 0;
    let argv = unsafe { CommandLineToArgvW(command.as_ptr(), &mut count) };
    if argv.is_null() || count <= 0 {
        return Err(io::Error::other("Invalid editor command"));
    }
    let mut words = Vec::new();
    for &word in unsafe { std::slice::from_raw_parts(argv, count as usize) } {
        let mut length = 0;
        unsafe {
            while *word.add(length) != 0 {
                length += 1;
            }
        }
        words.push(String::from_utf16_lossy(unsafe {
            std::slice::from_raw_parts(word, length)
        }));
    }
    unsafe {
        LocalFree(argv.cast());
    }
    let (program, args) = words
        .split_first()
        .ok_or_else(|| io::Error::other("Empty editor command"))?;
    if Command::new(program)
        .args(args)
        .arg(file)
        .status()?
        .success()
    {
        Ok(())
    } else {
        Err(io::Error::other("The developer's editor failed"))
    }
}
