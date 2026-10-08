use super::{ffi, ColorScheme};
use std::ffi::c_void;
use std::sync::Once;
use std::{ptr, slice};
static INSTALL_PNG_DECODER: Once = Once::new();

type WritePtyCallback = dyn FnMut(&[u8]) + Send;

pub(super) const MAX_CLIPBOARD_BYTES: usize = 192 * 1024;

#[derive(Default)]
pub(super) struct TerminalCallbackState {
    pub(super) write_pty: Option<Box<WritePtyCallback>>,
    pub(super) bell_count: u16,
    pub(super) pwd_changes: Vec<Vec<u8>>,
    pub(super) clipboard_writes: Vec<Vec<u8>>,
    pub(super) size_report: ffi::GhosttySizeReportSize,
    pub(super) color_scheme: Option<ColorScheme>,
}

pub(super) unsafe extern "C" fn bell_trampoline(
    _terminal: ffi::GhosttyTerminal,
    userdata: *mut c_void,
) {
    if userdata.is_null() {
        return;
    }
    // SAFETY: userdata is the TerminalCallbackState installed with this terminal.
    let state = unsafe { &mut *userdata.cast::<TerminalCallbackState>() };
    state.bell_count = state.bell_count.saturating_add(1);
}

pub(super) unsafe extern "C" fn color_scheme_trampoline(
    _terminal: ffi::GhosttyTerminal,
    userdata: *mut c_void,
    out_scheme: *mut ffi::GhosttyColorScheme,
) -> bool {
    if userdata.is_null() || out_scheme.is_null() {
        return false;
    }
    let state = unsafe { &*userdata.cast::<TerminalCallbackState>() };
    let Some(color_scheme) = state.color_scheme else {
        return false;
    };
    unsafe {
        out_scheme.write(color_scheme.as_raw());
    }
    true
}

pub(super) unsafe extern "C" fn size_trampoline(
    _terminal: ffi::GhosttyTerminal,
    userdata: *mut c_void,
    out_size: *mut ffi::GhosttySizeReportSize,
) -> bool {
    if userdata.is_null() || out_size.is_null() {
        return false;
    }
    let state = unsafe { &*userdata.cast::<TerminalCallbackState>() };
    let size = state.size_report;
    if size.rows == 0 || size.columns == 0 || size.cell_width == 0 || size.cell_height == 0 {
        return false;
    }
    unsafe {
        out_size.write(size);
    }
    true
}

pub(super) unsafe extern "C" fn write_pty_trampoline(
    _terminal: ffi::GhosttyTerminal,
    userdata: *mut c_void,
    data: *const u8,
    len: usize,
) {
    if userdata.is_null() || (data.is_null() && len != 0) {
        return;
    }
    let state = unsafe { &mut *(userdata.cast::<TerminalCallbackState>()) };
    let Some(callback) = state.write_pty.as_mut() else {
        return;
    };
    let bytes = if len == 0 {
        &[]
    } else {
        unsafe { slice::from_raw_parts(data, len) }
    };
    callback(bytes);
}

pub(super) unsafe extern "C" fn clipboard_write_trampoline(
    _terminal: ffi::GhosttyTerminal,
    userdata: *mut c_void,
    write: *const ffi::GhosttyClipboardWrite,
) -> ffi::GhosttyClipboardWriteResult {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: libghostty-vt owns these values for the synchronous callback.
        unsafe { capture_clipboard_write(userdata, write) }
    }))
    .unwrap_or(ffi::GhosttyClipboardWriteResult_GHOSTTY_CLIPBOARD_WRITE_RESULT_INVALID_DATA)
}

unsafe fn capture_clipboard_write(
    userdata: *mut c_void,
    write: *const ffi::GhosttyClipboardWrite,
) -> ffi::GhosttyClipboardWriteResult {
    if userdata.is_null() || write.is_null() {
        return ffi::GhosttyClipboardWriteResult_GHOSTTY_CLIPBOARD_WRITE_RESULT_INVALID_DATA;
    }

    // SAFETY: the statically linked libghostty-vt constructs the full request struct.
    let request = unsafe { &*write };
    if request.location != ffi::GhosttyClipboardLocation_GHOSTTY_CLIPBOARD_LOCATION_STANDARD {
        return ffi::GhosttyClipboardWriteResult_GHOSTTY_CLIPBOARD_WRITE_RESULT_UNSUPPORTED;
    }

    // SAFETY: userdata is the TerminalCallbackState installed with this terminal.
    let state = unsafe { &mut *userdata.cast::<TerminalCallbackState>() };
    if request.contents_len == 0 {
        return ffi::GhosttyClipboardWriteResult_GHOSTTY_CLIPBOARD_WRITE_RESULT_SUCCESS;
    }
    if request.contents_len != 1 {
        return ffi::GhosttyClipboardWriteResult_GHOSTTY_CLIPBOARD_WRITE_RESULT_UNSUPPORTED;
    }
    if request.contents.is_null() {
        return ffi::GhosttyClipboardWriteResult_GHOSTTY_CLIPBOARD_WRITE_RESULT_INVALID_DATA;
    }

    // SAFETY: libghostty-vt keeps the single content and its strings alive for the callback.
    let content = unsafe { &*request.contents };
    // SAFETY: the MIME string is borrowed from the live callback request.
    let Some(mime) = (unsafe { borrowed_bytes(content.mime) }) else {
        return ffi::GhosttyClipboardWriteResult_GHOSTTY_CLIPBOARD_WRITE_RESULT_INVALID_DATA;
    };
    let is_text = std::str::from_utf8(mime)
        .ok()
        .and_then(|mime| mime.split(';').next())
        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/plain"));
    if !is_text {
        return ffi::GhosttyClipboardWriteResult_GHOSTTY_CLIPBOARD_WRITE_RESULT_UNSUPPORTED;
    }

    // SAFETY: the data string is borrowed from the live callback request.
    let Some(bytes) = (unsafe { borrowed_bytes(content.data) }) else {
        return ffi::GhosttyClipboardWriteResult_GHOSTTY_CLIPBOARD_WRITE_RESULT_INVALID_DATA;
    };
    if bytes.is_empty() {
        return ffi::GhosttyClipboardWriteResult_GHOSTTY_CLIPBOARD_WRITE_RESULT_UNSUPPORTED;
    }
    if bytes.len() > MAX_CLIPBOARD_BYTES {
        return ffi::GhosttyClipboardWriteResult_GHOSTTY_CLIPBOARD_WRITE_RESULT_INVALID_DATA;
    }
    state.clipboard_writes.push(bytes.to_vec());
    ffi::GhosttyClipboardWriteResult_GHOSTTY_CLIPBOARD_WRITE_RESULT_SUCCESS
}

unsafe fn borrowed_bytes<'a>(value: ffi::GhosttyString) -> Option<&'a [u8]> {
    if value.len == 0 {
        Some(&[])
    } else if value.ptr.is_null() {
        None
    } else {
        // SAFETY: the callback contract keeps pointer and length valid until return.
        Some(unsafe { slice::from_raw_parts(value.ptr, value.len) })
    }
}

pub(super) unsafe extern "C" fn pwd_changed_trampoline(
    terminal: ffi::GhosttyTerminal,
    userdata: *mut c_void,
) {
    if terminal.is_null() || userdata.is_null() {
        return;
    }
    let mut pwd = ffi::GhosttyString::default();
    let result = unsafe {
        ffi::ghostty_terminal_get(
            terminal,
            ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_PWD,
            (&mut pwd as *mut ffi::GhosttyString).cast(),
        )
    };
    if result != ffi::GhosttyResult_GHOSTTY_SUCCESS || (pwd.ptr.is_null() && pwd.len != 0) {
        return;
    }
    let bytes = if pwd.len == 0 {
        Vec::new()
    } else {
        unsafe { slice::from_raw_parts(pwd.ptr, pwd.len) }.to_vec()
    };
    let state = unsafe { &mut *(userdata.cast::<TerminalCallbackState>()) };
    state.pwd_changes.push(bytes);
}

pub(super) fn install_png_decoder_once() {
    INSTALL_PNG_DECODER.call_once(|| unsafe {
        let _ = ffi::ghostty_sys_set(
            ffi::GhosttySysOption_GHOSTTY_SYS_OPT_DECODE_PNG,
            (decode_png_trampoline as *const ()).cast(),
        );
    });
}

unsafe extern "C" fn decode_png_trampoline(
    _userdata: *mut c_void,
    allocator: *const ffi::GhosttyAllocator,
    data: *const u8,
    data_len: usize,
    out: *mut ffi::GhosttySysImage,
) -> bool {
    if data.is_null() || out.is_null() {
        return false;
    }
    let bytes = unsafe { slice::from_raw_parts(data, data_len) };
    let Some(rgba) = decode_png_rgba(bytes) else {
        return false;
    };
    let ptr = unsafe { ffi::ghostty_alloc(allocator, rgba.data.len()) };
    if ptr.is_null() {
        return false;
    }
    unsafe {
        ptr::copy_nonoverlapping(rgba.data.as_ptr(), ptr, rgba.data.len());
        *out = ffi::GhosttySysImage {
            width: rgba.width,
            height: rgba.height,
            data: ptr,
            data_len: rgba.data.len(),
        };
    }
    true
}

struct DecodedPng {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

fn decode_png_rgba(bytes: &[u8]) -> Option<DecodedPng> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    let frame = &buf[..info.buffer_size()];
    if info.bit_depth != png::BitDepth::Eight {
        return None;
    }

    let data = match info.color_type {
        png::ColorType::Rgba => frame.to_vec(),
        png::ColorType::Rgb => {
            let mut out = Vec::with_capacity((info.width as usize) * (info.height as usize) * 4);
            for rgb in frame.chunks_exact(3) {
                out.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
            out
        }
        png::ColorType::Grayscale => {
            let mut out = Vec::with_capacity((info.width as usize) * (info.height as usize) * 4);
            for gray in frame {
                out.extend_from_slice(&[*gray, *gray, *gray, 255]);
            }
            out
        }
        png::ColorType::GrayscaleAlpha => {
            let mut out = Vec::with_capacity((info.width as usize) * (info.height as usize) * 4);
            for ga in frame.chunks_exact(2) {
                out.extend_from_slice(&[ga[0], ga[0], ga[0], ga[1]]);
            }
            out
        }
        png::ColorType::Indexed => return None,
    };

    Some(DecodedPng {
        width: info.width,
        height: info.height,
        data,
    })
}
