use crate::platform::windows::clipboard_image;
use crate::platform::windows::wide_null;
use crate::platform::ClipboardImage;
use std::mem::size_of;
use std::ptr::copy_nonoverlapping;
use std::ptr::null_mut;
use std::sync::LazyLock;
use std::time::Duration;
use windows_sys::Win32::Foundation::GlobalFree;
use windows_sys::Win32::System::Console::GetConsoleWindow;
use windows_sys::Win32::System::DataExchange::CloseClipboard;
use windows_sys::Win32::System::DataExchange::EmptyClipboard;
use windows_sys::Win32::System::DataExchange::GetClipboardData;
use windows_sys::Win32::System::DataExchange::OpenClipboard;
use windows_sys::Win32::System::DataExchange::RegisterClipboardFormatW;
use windows_sys::Win32::System::DataExchange::SetClipboardData;
use windows_sys::Win32::System::Memory::GlobalAlloc;
use windows_sys::Win32::System::Memory::GlobalLock;
use windows_sys::Win32::System::Memory::GlobalSize;
use windows_sys::Win32::System::Memory::GlobalUnlock;
use windows_sys::Win32::System::Memory::GMEM_MOVEABLE;
use windows_sys::Win32::System::Ole::CF_DIB;
use windows_sys::Win32::System::Ole::CF_DIBV5;
use windows_sys::Win32::System::Ole::CF_UNICODETEXT;
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::Shell::Shell_NotifyIconW;
use windows_sys::Win32::UI::Shell::NIF_ICON;
use windows_sys::Win32::UI::Shell::NIF_INFO;
use windows_sys::Win32::UI::Shell::NIF_TIP;
use windows_sys::Win32::UI::Shell::NIIF_INFO;
use windows_sys::Win32::UI::Shell::NIIF_NOSOUND;
use windows_sys::Win32::UI::Shell::NIM_ADD;
use windows_sys::Win32::UI::Shell::NIM_DELETE;
use windows_sys::Win32::UI::Shell::NIM_MODIFY;
use windows_sys::Win32::UI::Shell::NOTIFYICONDATAW;
use windows_sys::Win32::UI::WindowsAndMessaging::CreateWindowExW;
use windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::LoadIconW;
use windows_sys::Win32::UI::WindowsAndMessaging::IDI_APPLICATION;

pub fn write_clipboard(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    if text.contains('\0') {
        return false;
    }
    let mut utf16: Vec<u16> = text.encode_utf16().collect();
    utf16.push(0);
    let Some(byte_len) = utf16.len().checked_mul(size_of::<u16>()) else {
        return false;
    };

    unsafe {
        let owner = GetConsoleWindow();
        if owner.is_null() || OpenClipboard(owner) == 0 {
            return false;
        }
        let _clipboard = ClipboardGuard;

        if EmptyClipboard() == 0 {
            return false;
        }

        let memory = GlobalAlloc(GMEM_MOVEABLE, byte_len);
        if memory.is_null() {
            return false;
        }

        let locked = GlobalLock(memory);
        if locked.is_null() {
            GlobalFree(memory);
            return false;
        }
        copy_nonoverlapping(utf16.as_ptr(), locked.cast::<u16>(), utf16.len());
        GlobalUnlock(memory);

        if SetClipboardData(CF_UNICODETEXT as u32, memory).is_null() {
            GlobalFree(memory);
            return false;
        }

        true
    }
}

pub fn open_url(url: &str) -> std::io::Result<Option<std::process::Child>> {
    let operation = wide_null("open");
    let url = wide_null(url);
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            url.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
        )
    };
    if result as isize > 32 {
        Ok(None)
    } else {
        Err(std::io::Error::other(format!(
            "failed to open URL with ShellExecuteW: code {}",
            result as isize
        )))
    }
}

pub fn read_clipboard_image(max_bytes: usize) -> Option<ClipboardImage> {
    for attempt in 0..10 {
        if unsafe { OpenClipboard(null_mut()) } != 0 {
            let _clipboard = ClipboardGuard;
            if let Some(bytes) = read_registered_png_clipboard(max_bytes) {
                return Some(ClipboardImage {
                    bytes,
                    extension: "png",
                });
            }
            for format in [CF_DIBV5 as u32, CF_DIB as u32] {
                if let Some(bytes) =
                    clipboard_global_bytes(format, clipboard_image::MAX_CLIPBOARD_ALLOCATION)
                {
                    if let Some(bytes) = clipboard_image::dib_to_png(&bytes, max_bytes) {
                        return Some(ClipboardImage {
                            bytes,
                            extension: "png",
                        });
                    }
                }
            }
            return None;
        }
        if attempt < 9 {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    None
}

pub(in crate::platform::windows) fn read_registered_png_clipboard(
    max_bytes: usize,
) -> Option<Vec<u8>> {
    static PNG_FORMAT: LazyLock<u32> = LazyLock::new(|| {
        let name = wide_null("PNG");
        unsafe { RegisterClipboardFormatW(name.as_ptr()) }
    });
    if *PNG_FORMAT == 0 {
        return None;
    }
    let bytes = clipboard_global_bytes(*PNG_FORMAT, max_bytes.saturating_add(64 * 1024))?;
    clipboard_image::validated_png(&bytes, max_bytes)
}

pub(in crate::platform::windows) fn clipboard_global_bytes(
    format: u32,
    max_bytes: usize,
) -> Option<Vec<u8>> {
    let handle = unsafe { GetClipboardData(format) };
    if handle.is_null() {
        return None;
    }
    let data = unsafe { GlobalLock(handle) };
    if data.is_null() {
        return None;
    }
    let size = unsafe { GlobalSize(handle) };
    if size == 0 || size > max_bytes {
        unsafe {
            GlobalUnlock(handle);
        }
        return None;
    }
    let mut bytes = vec![0_u8; size];
    unsafe {
        copy_nonoverlapping(data.cast::<u8>(), bytes.as_mut_ptr(), size);
        GlobalUnlock(handle);
    }
    Some(bytes)
}

pub fn show_desktop_notification(title: &str, body: Option<&str>) -> std::io::Result<bool> {
    let title = title.to_owned();
    let body = body.unwrap_or(&title).to_owned();
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("bus-windows-notification".into())
        .spawn(move || show_desktop_notification_on_thread(&title, &body, ready_tx))?;
    ready_rx
        .recv_timeout(Duration::from_secs(2))
        .map_err(|err| match err {
            std::sync::mpsc::RecvTimeoutError::Timeout => std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "Windows notification setup timed out",
            ),
            std::sync::mpsc::RecvTimeoutError::Disconnected => std::io::Error::other(
                "Windows notification thread exited before reporting readiness",
            ),
        })?
}

pub(in crate::platform::windows) fn show_desktop_notification_on_thread(
    title: &str,
    body: &str,
    ready_tx: std::sync::mpsc::SyncSender<std::io::Result<bool>>,
) {
    let class_name = wide_null("STATIC");
    let window_name = wide_null("Bus notifications");
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            window_name.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            null_mut(),
            std::ptr::null(),
        )
    };
    if hwnd.is_null() {
        let _ = ready_tx.send(Err(std::io::Error::last_os_error()));
        return;
    }

    let mut notification = unsafe { std::mem::zeroed::<NOTIFYICONDATAW>() };
    notification.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    notification.hWnd = hwnd;
    notification.uID = 1;
    notification.hIcon = unsafe { LoadIconW(null_mut(), IDI_APPLICATION) };
    notification.uFlags = NIF_TIP;
    if !notification.hIcon.is_null() {
        notification.uFlags |= NIF_ICON;
    }
    copy_wide_truncated(&mut notification.szTip, "Bus");

    if unsafe { Shell_NotifyIconW(NIM_ADD, &notification) } == 0 {
        let _ = ready_tx.send(Err(std::io::Error::other(
            "failed to add Bus notification-area icon",
        )));
        unsafe {
            DestroyWindow(hwnd);
        }
        return;
    }

    notification.uFlags = NIF_INFO;
    notification.dwInfoFlags = NIIF_INFO | NIIF_NOSOUND;
    copy_wide_truncated(&mut notification.szInfoTitle, title);
    copy_wide_truncated(&mut notification.szInfo, body);
    if unsafe { Shell_NotifyIconW(NIM_MODIFY, &notification) } == 0 {
        unsafe {
            Shell_NotifyIconW(NIM_DELETE, &notification);
            DestroyWindow(hwnd);
        }
        let _ = ready_tx.send(Err(std::io::Error::other(
            "failed to show Bus desktop notification",
        )));
        return;
    }

    let _ = ready_tx.send(Ok(true));
    std::thread::sleep(Duration::from_secs(10));
    unsafe {
        Shell_NotifyIconW(NIM_DELETE, &notification);
        DestroyWindow(hwnd);
    }
}

pub(in crate::platform::windows) fn copy_wide_truncated<const N: usize>(
    destination: &mut [u16; N],
    value: &str,
) {
    destination.fill(0);
    let mut offset = 0;
    for ch in value.chars() {
        let mut units = [0; 2];
        let encoded = ch.encode_utf16(&mut units);
        if offset + encoded.len() >= N {
            break;
        }
        destination[offset..offset + encoded.len()].copy_from_slice(encoded);
        offset += encoded.len();
    }
}

pub(in crate::platform::windows) struct ClipboardGuard;

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        unsafe {
            CloseClipboard();
        }
    }
}
