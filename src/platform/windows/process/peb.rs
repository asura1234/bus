use crate::platform::windows::process::snapshot::WindowsProcessCommand;
use std::collections::HashMap;
use std::ffi::c_void;
use std::ffi::OsStr;
use std::mem::size_of;
use std::mem::MaybeUninit;
use std::ptr::null_mut;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering as AtomicOrdering;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use windows_sys::Wdk::System::Threading::NtQueryInformationProcess;
use windows_sys::Wdk::System::Threading::ProcessBasicInformation;
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Foundation::NTSTATUS;
use windows_sys::Win32::Foundation::STATUS_SUCCESS;
use windows_sys::Win32::Foundation::UNICODE_STRING;
use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows_sys::Win32::System::Memory::VirtualQueryEx;
use windows_sys::Win32::System::Memory::MEMORY_BASIC_INFORMATION;
use windows_sys::Win32::System::Threading::GetProcessTimes;
use windows_sys::Win32::System::Threading::OpenProcess;
use windows_sys::Win32::System::Threading::QueryFullProcessImageNameW;
use windows_sys::Win32::System::Threading::PROCESS_BASIC_INFORMATION;
use windows_sys::Win32::System::Threading::PROCESS_QUERY_INFORMATION;
use windows_sys::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION;
use windows_sys::Win32::System::Threading::PROCESS_VM_READ;
use windows_sys::Win32::UI::Shell::CommandLineToArgvW;

pub(in crate::platform::windows) const STILL_ACTIVE: u32 = 259;

pub(in crate::platform::windows) const PANE_RUNTIME_MARKER_ENV_VAR: &str = "HERDR_PANE_RUNTIME_ID";

pub(in crate::platform::windows) const MAX_PROCESS_ENVIRONMENT_BYTES: usize = 256 * 1024;

pub(in crate::platform::windows) const PROCESS_ENVIRONMENT_READ_CHUNK_BYTES: usize = 16 * 1024;

pub(in crate::platform::windows) const PROCESS_RUNTIME_MARKER_CACHE_CAPACITY: usize = 1_024;

pub(in crate::platform::windows) const PROCESS_RUNTIME_MARKER_CACHE_RETENTION: Duration =
    Duration::from_secs(60);

pub(in crate::platform::windows) const PROCESS_RUNTIME_MARKER_NEGATIVE_TTL: Duration =
    Duration::from_secs(1);

pub(in crate::platform::windows) static NEXT_PANE_RUNTIME_MARKER: AtomicU64 = AtomicU64::new(1);

pub(in crate::platform::windows) static PROCESS_RUNTIME_MARKER_CACHE: LazyLock<
    Mutex<HashMap<u32, CachedProcessRuntimeMarker>>,
> = LazyLock::new(|| Mutex::new(HashMap::new()));

pub(in crate::platform::windows) static GIT_BASH_PROCESS_CACHE: LazyLock<
    Mutex<HashMap<u32, CachedGitBashProcess>>,
> = LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Debug)]
pub(in crate::platform::windows) struct CachedProcessRuntimeMarker {
    pub(in crate::platform::windows) creation_time: u64,
    pub(in crate::platform::windows) marker: Option<String>,
    pub(in crate::platform::windows) cached_at: Instant,
    pub(in crate::platform::windows) last_used: Instant,
}

#[derive(Debug)]
pub(in crate::platform::windows) struct CachedGitBashProcess {
    pub(in crate::platform::windows) creation_time: u64,
    pub(in crate::platform::windows) is_git_bash: bool,
    pub(in crate::platform::windows) last_used: Instant,
}

pub(crate) fn apply_pane_runtime_marker_platform(command: &mut portable_pty::CommandBuilder) {
    if command_uses_git_bash(command) {
        command.env(PANE_RUNTIME_MARKER_ENV_VAR, next_pane_runtime_marker());
    }
}

pub(in crate::platform::windows) fn next_pane_runtime_marker() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let counter = NEXT_PANE_RUNTIME_MARKER.fetch_add(1, AtomicOrdering::Relaxed);
    format!("{:x}-{timestamp:x}-{counter:x}", std::process::id())
}

pub(in crate::platform::windows) fn read_process_command(
    pid: u32,
    name: &str,
) -> WindowsProcessCommand {
    let Some(process) =
        ProcessHandle::open(pid, PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ)
    else {
        let creation_time = ProcessHandle::open(pid, PROCESS_QUERY_LIMITED_INFORMATION)
            .and_then(|process| process_creation_time(process.0));
        return WindowsProcessCommand::from_cmdline(name, creation_time, None);
    };
    let creation_time = process_creation_time(process.0);
    let cmdline = read_process_parameters(process.0)
        .and_then(|parameters| read_unicode_string(process.0, parameters.command_line));
    WindowsProcessCommand::from_cmdline(name, creation_time, cmdline)
}

pub(in crate::platform::windows) fn process_is_git_bash(pid: u32) -> bool {
    let Some(process) = ProcessHandle::open(pid, PROCESS_QUERY_LIMITED_INFORMATION) else {
        return false;
    };
    let Some(creation_time) = process_creation_time(process.0) else {
        return false;
    };
    {
        let mut cache = GIT_BASH_PROCESS_CACHE
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        if let Some(cached) = cache.get_mut(&pid) {
            if cached.creation_time == creation_time {
                cached.last_used = Instant::now();
                return cached.is_git_bash;
            }
        }
    }

    let is_git_bash = process_executable_path(process.0)
        .as_deref()
        .is_some_and(|path| is_git_bash_executable_path(std::path::Path::new(path)));
    let mut cache = GIT_BASH_PROCESS_CACHE
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    if cache.len() >= PROCESS_RUNTIME_MARKER_CACHE_CAPACITY {
        cache.retain(|_, cached| {
            cached.last_used.elapsed() < PROCESS_RUNTIME_MARKER_CACHE_RETENTION
        });
        if cache.len() >= PROCESS_RUNTIME_MARKER_CACHE_CAPACITY {
            cache.clear();
        }
    }
    cache.insert(
        pid,
        CachedGitBashProcess {
            creation_time,
            is_git_bash,
            last_used: Instant::now(),
        },
    );
    is_git_bash
}

pub(in crate::platform::windows) fn process_executable_path(process: HANDLE) -> Option<String> {
    let mut path = vec![0_u16; 32_768];
    let mut len = path.len() as u32;
    if unsafe { QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut len) } == 0 {
        return None;
    }
    String::from_utf16(&path[..len as usize]).ok()
}

pub(in crate::platform::windows) fn command_uses_git_bash(
    command: &portable_pty::CommandBuilder,
) -> bool {
    let Some(program) = command.get_argv().first() else {
        return false;
    };
    let path = std::path::Path::new(program);
    if path.is_absolute() {
        return is_git_bash_executable_path(path);
    }
    if program.to_string_lossy().contains(['/', '\\']) {
        return false;
    }

    let Some(file_name) = path.file_name().and_then(OsStr::to_str) else {
        return false;
    };
    let candidate_name = if file_name.eq_ignore_ascii_case("bash") {
        "bash.exe"
    } else if file_name.eq_ignore_ascii_case("bash.exe") {
        file_name
    } else {
        return false;
    };
    let search_path = command
        .get_env("PATH")
        .map(OsStr::to_os_string)
        .or_else(|| std::env::var_os("PATH"));
    search_path.is_some_and(|search_path| {
        std::env::split_paths(&search_path)
            .map(|directory| directory.join(candidate_name))
            .find(|candidate| candidate.is_file())
            .is_some_and(|candidate| is_git_bash_executable_path(&candidate))
    })
}

pub(in crate::platform::windows) fn is_git_bash_executable_path(path: &std::path::Path) -> bool {
    let Some(file_name) = path.file_name().and_then(OsStr::to_str) else {
        return false;
    };
    if !file_name.eq_ignore_ascii_case("bash.exe") || !path.is_absolute() || !path.is_file() {
        return false;
    }

    let Some(bin_dir) = path.parent() else {
        return false;
    };
    if !bin_dir
        .file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| name.eq_ignore_ascii_case("bin"))
    {
        return false;
    }

    let Some(mut root) = bin_dir.parent() else {
        return false;
    };
    if root
        .file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| name.eq_ignore_ascii_case("usr"))
    {
        let Some(parent) = root.parent() else {
            return false;
        };
        root = parent;
    }

    root.join("usr").join("bin").join("msys-2.0.dll").is_file()
        && root.join("cmd").join("git.exe").is_file()
}

pub(in crate::platform::windows) fn process_runtime_marker(pid: u32) -> Option<String> {
    let process = ProcessHandle::open(pid, PROCESS_QUERY_INFORMATION | PROCESS_VM_READ)?;
    let creation_time = process_creation_time(process.0)?;
    {
        let mut cache = PROCESS_RUNTIME_MARKER_CACHE
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        if let Some(cached) = cache.get_mut(&pid) {
            if cached.creation_time == creation_time
                && (cached.marker.is_some()
                    || cached.cached_at.elapsed() < PROCESS_RUNTIME_MARKER_NEGATIVE_TTL)
            {
                cached.last_used = Instant::now();
                return cached.marker.clone();
            }
        }
    }

    let marker = process_runtime_marker_from_handle(process.0)?;
    let mut cache = PROCESS_RUNTIME_MARKER_CACHE
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    if cache.len() >= PROCESS_RUNTIME_MARKER_CACHE_CAPACITY {
        cache.retain(|_, cached| {
            cached.last_used.elapsed() < PROCESS_RUNTIME_MARKER_CACHE_RETENTION
        });
        if cache.len() >= PROCESS_RUNTIME_MARKER_CACHE_CAPACITY {
            cache.clear();
        }
    }
    cache.insert(
        pid,
        CachedProcessRuntimeMarker {
            creation_time,
            marker: marker.clone(),
            cached_at: Instant::now(),
            last_used: Instant::now(),
        },
    );
    marker
}

pub(in crate::platform::windows) fn process_creation_time(process: HANDLE) -> Option<u64> {
    let mut creation_time = FILETIME::default();
    let mut exit_time = FILETIME::default();
    let mut kernel_time = FILETIME::default();
    let mut user_time = FILETIME::default();
    if unsafe {
        GetProcessTimes(
            process,
            &mut creation_time,
            &mut exit_time,
            &mut kernel_time,
            &mut user_time,
        )
    } == 0
    {
        return None;
    }
    Some((u64::from(creation_time.dwHighDateTime) << 32) | u64::from(creation_time.dwLowDateTime))
}

pub(in crate::platform::windows) fn process_runtime_marker_from_handle(
    process: HANDLE,
) -> Option<Option<String>> {
    let parameters = read_process_parameters(process)?;
    let environment = read_process_environment(process, parameters.environment)?;
    Some(environment_variable_from_utf16(
        &environment,
        PANE_RUNTIME_MARKER_ENV_VAR,
    ))
}

pub(in crate::platform::windows) fn read_process_environment(
    process: HANDLE,
    address: *const c_void,
) -> Option<Vec<u16>> {
    if address.is_null() {
        return None;
    }

    let mut memory = MaybeUninit::<MEMORY_BASIC_INFORMATION>::uninit();
    let queried = unsafe {
        VirtualQueryEx(
            process,
            address,
            memory.as_mut_ptr(),
            size_of::<MEMORY_BASIC_INFORMATION>(),
        )
    };
    if queried == 0 {
        return None;
    }
    let memory = unsafe { memory.assume_init() };
    let address = address as usize;
    let base = memory.BaseAddress as usize;
    let offset = address.checked_sub(base)?;
    let available = memory.RegionSize.checked_sub(offset)?;
    let read_len = available.min(MAX_PROCESS_ENVIRONMENT_BYTES);
    if read_len < size_of::<u16>() {
        return None;
    }

    let max_units = read_len / size_of::<u16>();
    let chunk_units = PROCESS_ENVIRONMENT_READ_CHUNK_BYTES / size_of::<u16>();
    let mut environment = Vec::new();
    while environment.len() < max_units {
        let unit_count = (max_units - environment.len()).min(chunk_units);
        let chunk_bytes = unit_count * size_of::<u16>();
        let mut chunk = vec![0_u16; unit_count];
        let mut bytes_read = 0;
        let offset = environment.len().checked_mul(size_of::<u16>())?;
        let chunk_address = address.checked_add(offset)?;
        if unsafe {
            ReadProcessMemory(
                process,
                chunk_address as *const c_void,
                chunk.as_mut_ptr().cast::<c_void>(),
                chunk_bytes,
                &mut bytes_read,
            )
        } == 0
        {
            break;
        }
        chunk.truncate(bytes_read / size_of::<u16>());
        if chunk.is_empty() {
            break;
        }
        environment.extend_from_slice(&chunk);
        if let Some(end) = environment
            .windows(2)
            .position(|pair| pair == [0, 0])
            .map(|index| index + 2)
        {
            environment.truncate(end);
            return Some(environment);
        }
        if bytes_read < chunk_bytes {
            break;
        }
    }
    None
}

pub(in crate::platform::windows) fn environment_variable_from_utf16(
    environment: &[u16],
    name: &str,
) -> Option<String> {
    for variable in environment.split(|unit| *unit == 0) {
        if variable.is_empty() {
            break;
        }
        let Some(separator) = variable.iter().position(|unit| *unit == u16::from(b'=')) else {
            continue;
        };
        let Ok(variable_name) = String::from_utf16(&variable[..separator]) else {
            continue;
        };
        if variable_name.eq_ignore_ascii_case(name) {
            return String::from_utf16(&variable[separator + 1..]).ok();
        }
    }
    None
}

pub(in crate::platform::windows) fn read_process_parameters(
    process: HANDLE,
) -> Option<RtlUserProcessParameters> {
    let mut basic_info = MaybeUninit::<PROCESS_BASIC_INFORMATION>::uninit();
    let status = unsafe {
        NtQueryInformationProcess(
            process,
            ProcessBasicInformation,
            basic_info.as_mut_ptr().cast::<c_void>(),
            size_of::<PROCESS_BASIC_INFORMATION>() as u32,
            null_mut(),
        )
    };
    if status != STATUS_SUCCESS as NTSTATUS {
        return None;
    }

    let basic_info = unsafe { basic_info.assume_init() };
    if basic_info.PebBaseAddress.is_null() {
        return None;
    }

    let peb = read_process_value::<Peb>(process, basic_info.PebBaseAddress.cast::<c_void>())?;
    if peb.process_parameters.is_null() {
        return None;
    }

    read_process_value::<RtlUserProcessParameters>(process, peb.process_parameters.cast())
}

pub(in crate::platform::windows) fn command_line_to_argv(
    command_line: &str,
) -> Option<Vec<String>> {
    let wide: Vec<u16> = command_line
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut argc = 0;
    let argv_ptr = unsafe { CommandLineToArgvW(wide.as_ptr(), &mut argc) };
    if argv_ptr.is_null() || argc <= 0 {
        return None;
    }

    let argv_slice = unsafe { std::slice::from_raw_parts(argv_ptr, argc as usize) };
    let mut argv = Vec::with_capacity(argc as usize);
    for &arg in argv_slice {
        if arg.is_null() {
            continue;
        }
        let mut len = 0;
        unsafe {
            while *arg.add(len) != 0 {
                len += 1;
            }
            argv.push(String::from_utf16_lossy(std::slice::from_raw_parts(
                arg, len,
            )));
        }
    }
    unsafe {
        LocalFree(argv_ptr.cast());
    }
    Some(argv)
}

pub(in crate::platform::windows) fn nul_terminated_utf16_to_string(buffer: &[u16]) -> String {
    let len = buffer
        .iter()
        .position(|&value| value == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..len])
}

pub(in crate::platform::windows) struct ProcessHandle(pub(in crate::platform::windows) HANDLE);

impl ProcessHandle {
    pub(in crate::platform::windows) fn open(pid: u32, access: u32) -> Option<Self> {
        if pid == 0 {
            return None;
        }
        let handle = unsafe { OpenProcess(access, 0, pid) };
        (!handle.is_null()).then_some(Self(handle))
    }
}

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(in crate::platform::windows) struct Peb {
    pub(in crate::platform::windows) reserved1: [u8; 2],
    pub(in crate::platform::windows) being_debugged: u8,
    pub(in crate::platform::windows) reserved2: [u8; 1],
    pub(in crate::platform::windows) reserved3: [*mut c_void; 2],
    pub(in crate::platform::windows) ldr: *mut c_void,
    pub(in crate::platform::windows) process_parameters: *mut RtlUserProcessParameters,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(in crate::platform::windows) struct CurDir {
    pub(in crate::platform::windows) dos_path: UNICODE_STRING,
    pub(in crate::platform::windows) handle: HANDLE,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(in crate::platform::windows) struct RtlUserProcessParameters {
    pub(in crate::platform::windows) maximum_length: u32,
    pub(in crate::platform::windows) length: u32,
    pub(in crate::platform::windows) flags: u32,
    pub(in crate::platform::windows) debug_flags: u32,
    pub(in crate::platform::windows) console_handle: HANDLE,
    pub(in crate::platform::windows) console_flags: u32,
    pub(in crate::platform::windows) standard_input: HANDLE,
    pub(in crate::platform::windows) standard_output: HANDLE,
    pub(in crate::platform::windows) standard_error: HANDLE,
    pub(in crate::platform::windows) current_directory: CurDir,
    pub(in crate::platform::windows) dll_path: UNICODE_STRING,
    pub(in crate::platform::windows) image_path_name: UNICODE_STRING,
    pub(in crate::platform::windows) command_line: UNICODE_STRING,
    pub(in crate::platform::windows) environment: *mut c_void,
}

pub(in crate::platform::windows) fn read_process_value<T: Copy>(
    process: HANDLE,
    address: *const c_void,
) -> Option<T> {
    if address.is_null() {
        return None;
    }

    let mut value = MaybeUninit::<T>::uninit();
    let mut bytes_read = 0;
    let ok = unsafe {
        ReadProcessMemory(
            process,
            address,
            value.as_mut_ptr().cast::<c_void>(),
            size_of::<T>(),
            &mut bytes_read,
        )
    } != 0;

    (ok && bytes_read == size_of::<T>()).then(|| unsafe { value.assume_init() })
}

pub(in crate::platform::windows) fn read_unicode_string(
    process: HANDLE,
    unicode: UNICODE_STRING,
) -> Option<String> {
    if unicode.Buffer.is_null() || unicode.Length == 0 || !unicode.Length.is_multiple_of(2) {
        return None;
    }

    let char_len = usize::from(unicode.Length / 2);
    let mut buffer = vec![0_u16; char_len];
    let mut bytes_read = 0;
    let ok = unsafe {
        ReadProcessMemory(
            process,
            unicode.Buffer.cast::<c_void>(),
            buffer.as_mut_ptr().cast::<c_void>(),
            usize::from(unicode.Length),
            &mut bytes_read,
        )
    } != 0;

    if !ok || bytes_read != usize::from(unicode.Length) {
        return None;
    }

    String::from_utf16(&buffer).ok()
}
