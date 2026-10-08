use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet, VecDeque},
    ffi::{c_void, OsStr},
    mem::{size_of, MaybeUninit},
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    path::PathBuf,
    ptr::{copy_nonoverlapping, null_mut},
    sync::{
        atomic::{AtomicU64, Ordering as AtomicOrdering},
        Arc, LazyLock, Mutex, OnceLock,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

mod clipboard_image;

pub(crate) fn classify_child_exit(status: &portable_pty::ExitStatus) -> super::ChildExitReason {
    // STATUS_CONTROL_C_EXIT is reported without a Unix signal by portable-pty.
    if status.exit_code() == 0xC000013A {
        super::ChildExitReason::Interrupted
    } else {
        super::ChildExitReason::Exited
    }
}

pub(crate) fn wait_client_stream_readable(
    _stream: &crate::ipc::LocalStream,
) -> std::io::Result<()> {
    // Sync named pipes have no read timeout. The caller peeks before each read and checks its
    // cancellation flag between polls, including when a frame arrives in several fragments.
    std::thread::sleep(Duration::from_millis(2));
    Ok(())
}

pub(super) fn read_terminal_grid_size() -> std::io::Result<(u16, u16)> {
    crossterm::terminal::size()
}

pub(crate) fn replace_file(
    source: &std::path::Path,
    destination: &std::path::Path,
) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let moved = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

use windows_sys::{
    Wdk::System::Threading::{NtQueryInformationProcess, ProcessBasicInformation},
    Win32::{
        Foundation::{
            CloseHandle, GlobalFree, LocalFree, FILETIME, HANDLE, INVALID_HANDLE_VALUE, NTSTATUS,
            STATUS_SUCCESS, UNICODE_STRING,
        },
        Globalization::{CompareStringOrdinal, CSTR_EQUAL, CSTR_GREATER_THAN, CSTR_LESS_THAN},
        Security::SECURITY_ATTRIBUTES,
        Storage::FileSystem::CreateDirectoryW,
        System::{
            Console::GetConsoleWindow,
            DataExchange::{
                CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard,
                RegisterClipboardFormatW, SetClipboardData,
            },
            Diagnostics::{
                Debug::ReadProcessMemory,
                ToolHelp::{
                    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
                    TH32CS_SNAPPROCESS,
                },
            },
            JobObjects::{
                IsProcessInJob, JobObjectExtendedLimitInformation, QueryInformationJobObject,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            },
            Memory::{
                GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, VirtualQueryEx, GMEM_MOVEABLE,
                MEMORY_BASIC_INFORMATION,
            },
            Ole::{CF_DIB, CF_DIBV5, CF_UNICODETEXT},
            Threading::{
                GetCurrentProcess, GetExitCodeProcess, GetProcessTimes, OpenProcess,
                QueryFullProcessImageNameW, TerminateProcess, CREATE_NO_WINDOW, DETACHED_PROCESS,
                PROCESS_BASIC_INFORMATION, PROCESS_QUERY_INFORMATION,
                PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
            },
        },
        UI::{
            Input::KeyboardAndMouse::{GetKeyboardLayout, ToUnicodeEx},
            Shell::{
                CommandLineToArgvW, ShellExecuteW, Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_TIP,
                NIIF_INFO, NIIF_NOSOUND, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
            },
            WindowsAndMessaging::{
                CreateWindowExW, DestroyWindow, GetForegroundWindow, GetWindowThreadProcessId,
                LoadIconW, IDI_APPLICATION,
            },
        },
    },
};

use super::{ClipboardImage, ForegroundJob, Signal};

const STILL_ACTIVE: u32 = 259;
const FOREGROUND_PROCESS_SNAPSHOT_CACHE_TTL: Duration = Duration::from_millis(250);
const FOREGROUND_SELECTION_RECHECK: Duration = Duration::from_secs(5);
const FOREGROUND_SELECTION_CACHE_CAPACITY: usize = 1_024;
const FOREGROUND_SELECTION_CACHE_RETENTION: Duration = Duration::from_secs(60);
const PANE_RUNTIME_MARKER_ENV_VAR: &str = "HERDR_PANE_RUNTIME_ID";

pub(crate) fn terminal_title_for_presentation(title: &str) -> &str {
    title.strip_prefix("Administrator: ").unwrap_or(title)
}

pub(crate) fn prepare_paste_text_for_pty_platform(text: String) -> String {
    text.replace("\r\n", "\n").replace('\n', "\r\n")
}

/// Resolves against the current foreground layout because asynchronous console
/// records do not retain the layout that was active when the key was pressed.
pub(crate) fn resolve_base_printable_key(vk: u16, scan: u16) -> Option<char> {
    // SAFETY: Win32 owns the handles; the fixed buffers match the API lengths.
    unsafe {
        let thread_id = GetWindowThreadProcessId(GetForegroundWindow(), null_mut());
        let layout = GetKeyboardLayout(thread_id);

        let key_state = [0u8; 256];
        let mut output = [0u16; 2];
        let written = ToUnicodeEx(
            vk.into(),
            scan.into(),
            key_state.as_ptr(),
            output.as_mut_ptr(),
            output.len() as i32,
            0x4,
            layout,
        );
        let units = output.get(..usize::try_from(written).ok()?)?;
        let mut chars = char::decode_utf16(units.iter().copied());
        let ch = chars.next()?.ok()?;
        (chars.next().is_none() && !ch.is_control()).then_some(ch)
    }
}

const MAX_PROCESS_ENVIRONMENT_BYTES: usize = 256 * 1024;
const PROCESS_ENVIRONMENT_READ_CHUNK_BYTES: usize = 16 * 1024;
const PROCESS_RUNTIME_MARKER_CACHE_CAPACITY: usize = 1_024;
const PROCESS_RUNTIME_MARKER_CACHE_RETENTION: Duration = Duration::from_secs(60);
const PROCESS_RUNTIME_MARKER_NEGATIVE_TTL: Duration = Duration::from_secs(1);

static NEXT_PANE_RUNTIME_MARKER: AtomicU64 = AtomicU64::new(1);
static PROCESS_RUNTIME_MARKER_CACHE: LazyLock<Mutex<HashMap<u32, CachedProcessRuntimeMarker>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static GIT_BASH_PROCESS_CACHE: LazyLock<Mutex<HashMap<u32, CachedGitBashProcess>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub(crate) fn create_remote_ssh_config_file(
    path: &std::path::Path,
) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

pub(crate) fn create_remote_private_dir(path: &std::path::Path) -> std::io::Result<()> {
    use interprocess::os::windows::security_descriptor::{
        AsSecurityDescriptorExt as _, SecurityDescriptor,
    };
    use widestring::U16CString;

    let sddl = U16CString::from_str("D:P(A;OICI;GA;;;SY)(A;OICI;GA;;;OW)")
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
    let security_descriptor = SecurityDescriptor::deserialize(&sddl)?;
    let mut security_attributes = SECURITY_ATTRIBUTES {
        nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(u32::MAX),
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 0,
    };
    security_descriptor.write_to_security_attributes(&mut security_attributes);
    let path = extended_length_path(path)?;
    if unsafe { CreateDirectoryW(path.as_ptr(), &security_attributes) } != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn extended_length_path(path: &std::path::Path) -> std::io::Result<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt as _;

    let path = std::path::absolute(path)?;
    let wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
    let mut extended = if wide.starts_with(&[b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16])
        || wide.starts_with(&[b'\\' as u16, b'\\' as u16, b'.' as u16, b'\\' as u16])
    {
        wide
    } else if wide.starts_with(&[b'\\' as u16, b'\\' as u16]) {
        "\\\\?\\UNC\\"
            .encode_utf16()
            .chain(wide.into_iter().skip(2))
            .collect()
    } else {
        "\\\\?\\".encode_utf16().chain(wide).collect()
    };
    extended.push(0);
    Ok(extended)
}

pub(crate) fn remote_reattach_program(program: &str) -> String {
    let path = std::env::current_exe()
        .ok()
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| PathBuf::from(program));
    format!(
        "& {}",
        remote_reattach_argument(&path.display().to_string())
    )
}

fn remote_reattach_argument(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Encode native or targeted semantic Win32 input for a compatible ConPTY destination.
pub(crate) fn encode_windows_conpty_fallback(key: &crate::input::TerminalKey) -> Option<Vec<u8>> {
    use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};

    let (virtual_key_code, virtual_scan_code, unicode, control_key_state) =
        if let Some(record) = key.windows_record() {
            (
                record.virtual_key_code,
                record.virtual_scan_code,
                record.unicode,
                record.control_key_state,
            )
        } else if key.code == KeyCode::Esc
            && key.modifiers.is_empty()
            && key.kind == KeyEventKind::Press
            && key.vt_bytes().is_none()
        {
            return Some(b"\x1b[27;1;27;1;0;1_\x1b[27;1;27;0;0;1_".to_vec());
        } else if key.code == KeyCode::Enter && key.modifiers == KeyModifiers::SHIFT {
            (13, 28, 13, 16)
        } else {
            return None;
        };
    let key_down = key.kind != KeyEventKind::Release;
    let repeat_count = if key_down { key.repeat_count.max(1) } else { 1 };

    Some(
        format!(
            "\x1b[{virtual_key_code};{virtual_scan_code};{unicode};{};{control_key_state};{repeat_count}_",
            u8::from(key_down),
        )
        .into_bytes(),
    )
}

#[derive(Debug)]
struct CachedProcessSnapshot {
    built_at: Instant,
    snapshot: Arc<ProcessSnapshot>,
}

#[derive(Debug)]
struct ProcessSnapshotCache {
    cached: Option<CachedProcessSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcessSignature {
    pid: u32,
    parent_pid: u32,
    name: String,
}

impl ProcessSignature {
    fn from_entry(entry: &WindowsProcessEntry) -> Self {
        Self {
            pid: entry.pid,
            parent_pid: entry.parent_pid,
            name: entry.name.clone(),
        }
    }

    fn matches(&self, entry: Option<&WindowsProcessEntry>) -> bool {
        entry.is_some_and(|entry| {
            self.pid == entry.pid && self.parent_pid == entry.parent_pid && self.name == entry.name
        })
    }
}

#[derive(Debug)]
enum ProcessIdentity {
    Handle(OwnedHandle),
    #[cfg(test)]
    Stub {
        running: bool,
        creation_time: Option<u64>,
    },
}

impl ProcessIdentity {
    fn open(pid: u32) -> Option<Self> {
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return None;
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(handle.cast()) };
        Some(Self::Handle(handle))
    }

    fn running(&self) -> bool {
        match self {
            Self::Handle(handle) => {
                let mut exit_code = 0;
                let read = unsafe {
                    GetExitCodeProcess(handle.as_raw_handle().cast(), &mut exit_code) != 0
                };
                read && exit_code == STILL_ACTIVE
            }
            #[cfg(test)]
            Self::Stub { running, .. } => *running,
        }
    }

    fn creation_time(&self) -> Option<u64> {
        match self {
            Self::Handle(handle) => process_creation_time(handle.as_raw_handle().cast()),
            #[cfg(test)]
            Self::Stub { creation_time, .. } => *creation_time,
        }
    }
}

#[derive(Debug)]
struct CachedForegroundSelection {
    shell: ProcessSignature,
    selected: ProcessSignature,
    descendants: Vec<ProcessSignature>,
    descendant_identities: Vec<ProcessIdentity>,
    shell_identity: ProcessIdentity,
    selected_identity: ProcessIdentity,
    job: ForegroundJob,
    verified_at: Instant,
    last_used: Instant,
}

#[derive(Debug, Default)]
struct ForegroundSelectionCache {
    entries: HashMap<u32, CachedForegroundSelection>,
}

#[derive(Debug)]
struct CachedProcessRuntimeMarker {
    creation_time: u64,
    marker: Option<String>,
    cached_at: Instant,
    last_used: Instant,
}

#[derive(Debug)]
struct CachedGitBashProcess {
    creation_time: u64,
    is_git_bash: bool,
    last_used: Instant,
}

static FOREGROUND_PROCESS_SNAPSHOT_CACHE: Mutex<ProcessSnapshotCache> =
    Mutex::new(ProcessSnapshotCache { cached: None });
static FOREGROUND_SELECTION_CACHE: LazyLock<Mutex<ForegroundSelectionCache>> =
    LazyLock::new(|| Mutex::new(ForegroundSelectionCache::default()));

pub(crate) fn should_draw_host_cursor_by_default() -> bool {
    true
}

pub(crate) fn should_query_host_terminal_palette() -> bool {
    false
}

/// The machine's node name, as shown by tmux's `#h`.
pub(crate) fn hostname() -> Option<String> {
    std::env::var("COMPUTERNAME")
        .ok()
        .filter(|name| !name.is_empty())
}

#[cfg(test)]
pub(crate) fn local_datetime() -> Option<time::PrimitiveDateTime> {
    let mut timestamp: libc::time_t = 0;
    if unsafe { libc::time(&mut timestamp) } == -1 {
        return None;
    }
    local_datetime_at(timestamp)
}

pub(crate) fn local_datetime_at(seconds: i64) -> Option<time::PrimitiveDateTime> {
    let timestamp = libc::time_t::try_from(seconds).ok()?;
    let mut local: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_s(&mut local, &timestamp) } != 0 {
        return None;
    }
    let month = time::Month::try_from(u8::try_from(local.tm_mon + 1).ok()?).ok()?;
    let date = time::Date::from_calendar_date(
        local.tm_year + 1900,
        month,
        u8::try_from(local.tm_mday).ok()?,
    )
    .ok()?;
    let time = time::Time::from_hms(
        u8::try_from(local.tm_hour).ok()?,
        u8::try_from(local.tm_min).ok()?,
        u8::try_from(local.tm_sec).ok()?,
    )
    .ok()?;
    Some(time::PrimitiveDateTime::new(date, time))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WindowsProcessCommand {
    creation_time: Option<u64>,
    argv0: Option<String>,
    argv: Option<Vec<String>>,
    cmdline: Option<String>,
}

#[derive(Debug, Clone)]
struct WindowsProcessEntry {
    pid: u32,
    parent_pid: u32,
    name: String,
    command: OnceLock<WindowsProcessCommand>,
}

impl WindowsProcessEntry {
    fn command(&self) -> &WindowsProcessCommand {
        self.command
            .get_or_init(|| read_process_command(self.pid, &self.name))
    }
}

#[derive(Debug)]
struct ProcessSnapshot {
    entries: Vec<WindowsProcessEntry>,
    entry_by_pid: HashMap<u32, usize>,
    children_by_parent: HashMap<u32, Vec<usize>>,
    agent_indices: OnceLock<Vec<usize>>,
}

impl ProcessSnapshot {
    fn new(entries: Vec<WindowsProcessEntry>) -> Self {
        let mut entry_by_pid = HashMap::with_capacity(entries.len());
        let mut children_by_parent = HashMap::<u32, Vec<usize>>::new();
        for (index, entry) in entries.iter().enumerate() {
            entry_by_pid.insert(entry.pid, index);
            children_by_parent
                .entry(entry.parent_pid)
                .or_default()
                .push(index);
        }
        Self {
            entries,
            entry_by_pid,
            children_by_parent,
            agent_indices: OnceLock::new(),
        }
    }

    fn entry(&self, pid: u32) -> Option<&WindowsProcessEntry> {
        self.entry_by_pid
            .get(&pid)
            .map(|&index| &self.entries[index])
    }

    fn descendant_signatures(&self, root_pid: u32) -> Vec<ProcessSignature> {
        let mut signatures = descendant_entries(root_pid, self)
            .into_iter()
            .map(ProcessSignature::from_entry)
            .collect::<Vec<_>>();
        signatures.sort_unstable_by_key(|entry| entry.pid);
        signatures
    }

    fn agent_indices(&self) -> &[usize] {
        self.agent_indices.get_or_init(|| {
            self.entries
                .iter()
                .enumerate()
                .filter_map(|(index, entry)| process_entry_identifies_agent(entry).then_some(index))
                .collect()
        })
    }
}

pub fn raise_server_nofile_limit() {}

pub(crate) fn apply_pane_runtime_marker_platform(command: &mut portable_pty::CommandBuilder) {
    if command_uses_git_bash(command) {
        command.env(PANE_RUNTIME_MARKER_ENV_VAR, next_pane_runtime_marker());
    }
}

fn next_pane_runtime_marker() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let counter = NEXT_PANE_RUNTIME_MARKER.fetch_add(1, AtomicOrdering::Relaxed);
    format!("{:x}-{timestamp:x}-{counter:x}", std::process::id())
}

pub(crate) fn interactive_shell_command(argv: &[String], shell_name: &str) -> Option<String> {
    let shell_name = shell_name.to_ascii_lowercase();
    let powershell = shell_name.contains("powershell") || shell_name.contains("pwsh");
    let script = powershell_agent_script(argv)?;
    if powershell {
        Some(script)
    } else {
        Some(cmd_encoded_powershell_command(&script))
    }
}

fn powershell_agent_script(argv: &[String]) -> Option<String> {
    let (program, args) = argv.split_first()?;
    if args.is_empty() {
        return Some(format!("& {}", super::quote_powershell_arg(program)));
    }

    let powershell_args = args
        .iter()
        .map(|arg| super::quote_powershell_arg(arg))
        .collect::<Vec<_>>()
        .join(" ");
    let command_line = args
        .iter()
        .map(|arg| quote_windows_command_line_arg(arg))
        .collect::<Vec<_>>()
        .join(" ");
    Some(format!(
        "if((Get-Command {} -ErrorAction SilentlyContinue).CommandType -eq 'ExternalScript'){{& {} {}}}else{{Start-Process -FilePath {} -ArgumentList {} -NoNewWindow -Wait}}",
        super::quote_powershell_arg(program),
        super::quote_powershell_arg(program),
        powershell_args,
        super::quote_powershell_arg(program),
        super::quote_powershell_arg(&command_line),
    ))
}

fn quote_windows_command_line_arg(value: &str) -> String {
    if !value.is_empty()
        && !value
            .chars()
            .any(|ch| matches!(ch, ' ' | '\t' | '\n' | '\x0b' | '"'))
    {
        return value.to_string();
    }

    let mut quoted = String::from("\"");
    let mut backslashes = 0;
    for ch in value.chars() {
        if ch == '\\' {
            backslashes += 1;
            continue;
        }
        if ch == '"' {
            quoted.push_str(&"\\".repeat(backslashes * 2 + 1));
        } else {
            quoted.push_str(&"\\".repeat(backslashes));
        }
        backslashes = 0;
        quoted.push(ch);
    }
    quoted.push_str(&"\\".repeat(backslashes * 2));
    quoted.push('"');
    quoted
}

fn cmd_encoded_powershell_command(script: &str) -> String {
    use base64::Engine as _;

    let utf16 = script
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    let encoded = base64::engine::general_purpose::STANDARD.encode(utf16);
    format!("powershell.exe -NoLogo -NoProfile -EncodedCommand {encoded}")
}

pub(crate) fn scrollback_editor_argv(path: &std::path::Path) -> std::io::Result<Vec<String>> {
    let editor = std::env::var("VISUAL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            std::env::var("EDITOR")
                .ok()
                .filter(|value| !value.trim().is_empty())
        });
    scrollback_editor_argv_with_env(path, editor.as_deref())
}

fn scrollback_editor_argv_with_env(
    path: &std::path::Path,
    editor: Option<&str>,
) -> std::io::Result<Vec<String>> {
    let mut argv = match editor.filter(|value| !value.trim().is_empty()) {
        Some(editor) => command_line_to_argv(editor).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("failed to parse editor command {editor:?}"),
            )
        })?,
        None => vec!["notepad.exe".to_string()],
    };
    if argv.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "editor command must not be empty",
        ));
    }
    argv.push(path.display().to_string());
    Ok(argv)
}

pub(crate) fn configure_background_command_platform(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;

    command.creation_flags(CREATE_NO_WINDOW);
}

pub fn launch_server_daemon_command(command: &mut std::process::Command) -> std::io::Result<u32> {
    if current_job_kills_processes_on_close()? {
        launch_server_daemon_with_wmi(command)
    } else {
        command.spawn().map(|child| child.id())
    }
}

fn launch_server_daemon_with_wmi(command: &std::process::Command) -> std::io::Result<u32> {
    // WMI resolves the class from this Rust type name, including CIM casing.
    #[allow(non_camel_case_types)]
    #[derive(serde::Deserialize)]
    struct Win32_Process;

    // WMI serializes this embedded object using the matching CIM class name.
    #[allow(non_camel_case_types)]
    #[derive(serde::Serialize)]
    struct Win32_ProcessStartup {
        #[serde(rename = "CreateFlags")]
        create_flags: u32,
        #[serde(rename = "EnvironmentVariables")]
        environment_variables: Vec<String>,
    }

    #[derive(serde::Serialize)]
    struct CreateInput {
        #[serde(rename = "CommandLine")]
        command_line: String,
        #[serde(rename = "CurrentDirectory")]
        current_directory: String,
        #[serde(rename = "ProcessStartupInformation")]
        process_startup_information: Win32_ProcessStartup,
    }

    #[derive(serde::Deserialize)]
    struct CreateOutput {
        #[serde(rename = "ProcessId")]
        process_id: Option<u32>,
        #[serde(rename = "ReturnValue")]
        return_value: u32,
    }

    let current_directory = command
        .get_current_dir()
        .map(std::path::Path::to_path_buf)
        .map(Ok)
        .unwrap_or_else(std::env::current_dir)?;
    let input = CreateInput {
        command_line: windows_command_line(command)?,
        current_directory: unicode_windows_value(
            &current_directory.into_os_string(),
            "working directory",
        )?,
        process_startup_information: Win32_ProcessStartup {
            create_flags: DETACHED_PROCESS,
            environment_variables: effective_command_environment(command)?,
        },
    };

    let connection = wmi::WMIConnection::new()
        .map_err(|err| std::io::Error::other(format!("failed to connect to WMI: {err}")))?;
    let output: CreateOutput = connection
        .exec_class_method::<Win32_Process, _>("Create", &input)
        .map_err(|err| std::io::Error::other(format!("WMI Win32_Process.Create failed: {err}")))?;
    if output.return_value != 0 {
        return Err(std::io::Error::other(format!(
            "WMI Win32_Process.Create returned error {}",
            output.return_value
        )));
    }
    output.process_id.ok_or_else(|| {
        std::io::Error::other("WMI Win32_Process.Create succeeded without a process id")
    })
}

fn windows_command_line(command: &std::process::Command) -> std::io::Result<String> {
    std::iter::once(command.get_program())
        .chain(command.get_args())
        .map(|value| {
            unicode_windows_value(value, "server command argument")
                .map(|value| quote_windows_command_line_arg(&value))
        })
        .collect::<std::io::Result<Vec<_>>>()
        .map(|parts| parts.join(" "))
}

fn effective_command_environment(command: &std::process::Command) -> std::io::Result<Vec<String>> {
    let mut environment = std::env::vars_os()
        .map(|(key, value)| {
            Ok((
                unicode_windows_value(&key, "inherited environment variable name")?,
                unicode_windows_value(&value, "inherited environment variable value")?,
            ))
        })
        .collect::<std::io::Result<Vec<(String, String)>>>()?;
    for (key, value) in command.get_envs() {
        let key = unicode_windows_value(key, "environment variable name")?;
        environment.retain(|(inherited, _)| windows_environment_key_cmp(inherited, &key).is_ne());
        if let Some(value) = value {
            environment.push((
                key,
                unicode_windows_value(value, "environment variable value")?,
            ));
        }
    }
    environment.sort_unstable_by(|(left, _), (right, _)| windows_environment_key_cmp(left, right));
    Ok(environment
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect())
}

fn windows_environment_key_cmp(left: &str, right: &str) -> Ordering {
    let left_wide: Vec<u16> = left.encode_utf16().collect();
    let right_wide: Vec<u16> = right.encode_utf16().collect();
    // SAFETY: both pointers remain valid for the call and lengths count UTF-16 units.
    match unsafe {
        CompareStringOrdinal(
            left_wide.as_ptr(),
            left_wide.len() as i32,
            right_wide.as_ptr(),
            right_wide.len() as i32,
            1,
        )
    } {
        CSTR_LESS_THAN => Ordering::Less,
        CSTR_EQUAL => Ordering::Equal,
        CSTR_GREATER_THAN => Ordering::Greater,
        _ => left.cmp(right),
    }
}

fn unicode_windows_value(value: &OsStr, label: &str) -> std::io::Result<String> {
    value.to_str().map(str::to_owned).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{label} is not valid Unicode"),
        )
    })
}

fn current_process_is_in_job() -> std::io::Result<bool> {
    let mut in_job = 0;
    // SAFETY: `in_job` is a valid writable BOOL for the duration of the call.
    if unsafe { IsProcessInJob(GetCurrentProcess(), null_mut(), &mut in_job) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(in_job != 0)
}

fn current_job_kills_processes_on_close() -> std::io::Result<bool> {
    if !current_process_is_in_job()? {
        return Ok(false);
    }

    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    // SAFETY: `limits` is writable and its exact buffer size is supplied.
    if unsafe {
        QueryInformationJobObject(
            null_mut(),
            JobObjectExtendedLimitInformation,
            &mut limits as *mut _ as *mut c_void,
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            null_mut(),
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    Ok(limits.BasicLimitInformation.LimitFlags & JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE != 0)
}

pub fn detach_server_daemon_command(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;

    command.creation_flags(DETACHED_PROCESS);
}

pub fn current_process_is_detached_server_daemon() -> bool {
    if !unsafe { GetConsoleWindow() }.is_null() {
        return false;
    }

    matches!(current_process_is_in_job(), Ok(false))
}

pub fn foreground_job(child_pid: u32) -> Option<ForegroundJob> {
    select_pane_foreground_job_cached(child_pid)
}

pub(crate) fn available_pane_shell(child_pid: u32) -> Option<String> {
    let snapshot = ProcessSnapshot::new(snapshot_processes());
    available_pane_shell_from_snapshot(child_pid, &snapshot)
}

fn available_pane_shell_from_snapshot(
    child_pid: u32,
    snapshot: &ProcessSnapshot,
) -> Option<String> {
    let shell = snapshot.entry(child_pid)?;
    if !super::is_pane_shell_process_name(&shell.name) {
        return None;
    }
    descendant_entries(child_pid, snapshot)
        .is_empty()
        .then(|| shell.name.clone())
}

pub fn foreground_group_leader_job(process_group_id: u32) -> Option<ForegroundJob> {
    let snapshot = cached_foreground_processes();
    let entry = snapshot.entry(process_group_id)?;
    Some(ForegroundJob {
        process_group_id,
        processes: vec![foreground_process_from_entry(entry)],
    })
}

pub fn foreground_process_group_id(child_pid: u32) -> Option<u32> {
    select_pane_foreground_job_cached(child_pid).map(|job| job.process_group_id)
}

pub fn process_cwd(pid: u32) -> Option<PathBuf> {
    let process = ProcessHandle::open(pid, PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ)?;
    let process_parameters = read_process_parameters(process.0)?;
    read_unicode_string(process.0, process_parameters.current_directory.dos_path)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

fn select_pane_foreground_job_cached(shell_pid: u32) -> Option<ForegroundJob> {
    let snapshot = cached_foreground_processes();
    let (job, retry_with_fresh_snapshot) =
        select_pane_foreground_job_from_snapshot(shell_pid, &snapshot)?;
    if !retry_with_fresh_snapshot {
        return Some(job);
    }

    let snapshot = fresh_foreground_processes();
    select_pane_foreground_job_from_snapshot(shell_pid, &snapshot).map(|(job, _)| job)
}

fn select_pane_foreground_job_from_snapshot(
    shell_pid: u32,
    snapshot: &ProcessSnapshot,
) -> Option<(ForegroundJob, bool)> {
    if let Some(job) = FOREGROUND_SELECTION_CACHE
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .get(shell_pid, snapshot)
    {
        return Some((job, false));
    }

    let job = select_pane_foreground_job_from_snapshot_uncached(shell_pid, snapshot)?;
    let cached = prepare_cached_foreground_selection(shell_pid, snapshot, &job);
    let retry_with_fresh_snapshot = job.process_group_id != shell_pid && cached.is_none();
    FOREGROUND_SELECTION_CACHE
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .remember(shell_pid, cached);
    Some((job, retry_with_fresh_snapshot))
}

fn select_pane_foreground_job_from_snapshot_uncached(
    shell_pid: u32,
    snapshot: &ProcessSnapshot,
) -> Option<ForegroundJob> {
    select_pane_foreground_job_from_snapshot_with_runtime_inspection(
        shell_pid,
        snapshot,
        |shell| process_is_git_bash(shell.pid),
        |entry| process_runtime_marker(entry.pid),
    )
}

fn select_pane_foreground_job_from_snapshot_with_runtime_inspection(
    shell_pid: u32,
    snapshot: &ProcessSnapshot,
    shell_is_git_bash: impl FnOnce(&WindowsProcessEntry) -> bool,
    mut runtime_marker: impl FnMut(&WindowsProcessEntry) -> Option<String>,
) -> Option<ForegroundJob> {
    let entries = &snapshot.entries;
    let shell = snapshot.entry(shell_pid)?;
    let descendants = descendant_entries(shell_pid, snapshot);
    let mut candidates = Vec::new();
    for entry in std::iter::once(shell).chain(descendants) {
        if process_entry_identifies_agent(entry) {
            candidates.push(entry);
        }
    }

    if let Some(selected) = select_topmost_agent_chain_candidate(&candidates, snapshot) {
        return Some(foreground_job_from_entry(selected));
    }
    if !candidates.is_empty() || !shell_is_git_bash(shell) {
        return Some(foreground_job_from_entry(shell));
    }

    let escaped_agent_indices = snapshot.agent_indices();
    if escaped_agent_indices.is_empty() {
        return Some(foreground_job_from_entry(shell));
    }

    let Some(shell_runtime_marker) = runtime_marker(shell).filter(|marker| !marker.is_empty())
    else {
        return Some(foreground_job_from_entry(shell));
    };
    let matching_candidates: Vec<_> = escaped_agent_indices
        .iter()
        .map(|&index| &entries[index])
        .filter(|entry| runtime_marker(entry).as_deref() == Some(shell_runtime_marker.as_str()))
        .collect();
    let selected =
        select_topmost_agent_chain_candidate(&matching_candidates, snapshot).unwrap_or(shell);
    Some(foreground_job_from_entry(selected))
}

#[cfg(test)]
fn select_pane_foreground_job(
    shell_pid: u32,
    entries: &[WindowsProcessEntry],
) -> Option<ForegroundJob> {
    select_pane_foreground_job_from_snapshot_uncached(
        shell_pid,
        &ProcessSnapshot::new(entries.to_vec()),
    )
}

fn process_entry_identifies_agent(entry: &WindowsProcessEntry) -> bool {
    crate::detect::identify_agent(&entry.name).is_some()
        || crate::detect::identify_agent_in_job(&foreground_job_from_entry(entry)).is_some()
}

fn foreground_job_from_entry(entry: &WindowsProcessEntry) -> ForegroundJob {
    ForegroundJob {
        process_group_id: entry.pid,
        processes: vec![foreground_process_from_entry(entry)],
    }
}

fn select_topmost_agent_chain_candidate<'a>(
    candidates: &[&'a WindowsProcessEntry],
    snapshot: &ProcessSnapshot,
) -> Option<&'a WindowsProcessEntry> {
    if candidates.is_empty() {
        return None;
    }

    candidates.iter().copied().find(|entry| {
        candidates.iter().all(|other| {
            entry.pid == other.pid || process_is_ancestor(entry.pid, other.pid, snapshot)
        })
    })
}

fn process_is_ancestor(ancestor_pid: u32, descendant_pid: u32, snapshot: &ProcessSnapshot) -> bool {
    let mut current = descendant_pid;
    let mut visited = HashSet::new();
    while visited.insert(current) {
        let Some(parent) = snapshot.entry(current).map(|entry| entry.parent_pid) else {
            return false;
        };
        if parent == ancestor_pid {
            return true;
        }
        if parent == 0 {
            return false;
        }
        current = parent;
    }

    false
}

fn descendant_entries(root_pid: u32, snapshot: &ProcessSnapshot) -> Vec<&WindowsProcessEntry> {
    let mut output = Vec::new();
    let mut queue = VecDeque::new();
    let mut visited = HashSet::new();
    visited.insert(root_pid);
    if let Some(root_children) = snapshot.children_by_parent.get(&root_pid) {
        for &index in root_children {
            let entry = &snapshot.entries[index];
            if visited.insert(entry.pid) {
                queue.push_back(entry);
            }
        }
    }
    while let Some(entry) = queue.pop_front() {
        output.push(entry);
        if let Some(next) = snapshot.children_by_parent.get(&entry.pid) {
            for &index in next {
                let child = &snapshot.entries[index];
                if visited.insert(child.pid) {
                    queue.push_back(child);
                }
            }
        }
    }
    output
}

fn foreground_process_from_entry(entry: &WindowsProcessEntry) -> super::ForegroundProcess {
    let command = entry.command();
    super::ForegroundProcess {
        pid: entry.pid,
        name: entry.name.clone(),
        argv0: command.argv0.clone(),
        argv: command.argv.clone(),
        cmdline: command.cmdline.clone(),
    }
}

fn snapshot_processes() -> Vec<WindowsProcessEntry> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Vec::new();
    }
    let _snapshot = ProcessHandle(snapshot);

    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut output = Vec::new();
    let mut ok = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    while ok {
        let pid = entry.th32ProcessID;
        let name = nul_terminated_utf16_to_string(&entry.szExeFile);
        output.push(WindowsProcessEntry {
            pid,
            parent_pid: entry.th32ParentProcessID,
            name,
            command: OnceLock::new(),
        });
        ok = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    output
}

fn cached_foreground_processes() -> Arc<ProcessSnapshot> {
    let mut cache = FOREGROUND_PROCESS_SNAPSHOT_CACHE
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    cache.snapshot(FOREGROUND_PROCESS_SNAPSHOT_CACHE_TTL, snapshot_processes)
}

fn fresh_foreground_processes() -> Arc<ProcessSnapshot> {
    let mut cache = FOREGROUND_PROCESS_SNAPSHOT_CACHE
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    cache.snapshot(Duration::ZERO, snapshot_processes)
}

fn prepare_cached_foreground_selection(
    shell_pid: u32,
    snapshot: &ProcessSnapshot,
    job: &ForegroundJob,
) -> Option<CachedForegroundSelection> {
    if job.process_group_id == shell_pid {
        return None;
    }
    let shell_identity = ProcessIdentity::open(shell_pid)?;
    let selected_identity = ProcessIdentity::open(job.process_group_id)?;
    let descendants = snapshot.descendant_signatures(shell_pid);
    let descendant_identities = descendants
        .iter()
        .map(|entry| ProcessIdentity::open(entry.pid))
        .collect::<Option<Vec<_>>>()?;
    CachedForegroundSelection::from_snapshot_with_identities(
        shell_pid,
        snapshot,
        job,
        descendants,
        descendant_identities,
        shell_identity,
        selected_identity,
    )
}

impl CachedForegroundSelection {
    fn from_snapshot_with_identities(
        shell_pid: u32,
        snapshot: &ProcessSnapshot,
        job: &ForegroundJob,
        descendants: Vec<ProcessSignature>,
        descendant_identities: Vec<ProcessIdentity>,
        shell_identity: ProcessIdentity,
        selected_identity: ProcessIdentity,
    ) -> Option<Self> {
        if job.process_group_id == shell_pid {
            return None;
        }
        let shell_entry = snapshot.entry(shell_pid)?;
        if shell_identity.creation_time() != shell_entry.command().creation_time {
            return None;
        }
        let shell = ProcessSignature::from_entry(shell_entry);
        let selected_entry = snapshot.entry(job.process_group_id)?;
        if selected_identity.creation_time() != selected_entry.command().creation_time {
            return None;
        }
        let selected = ProcessSignature::from_entry(selected_entry);
        let descendants_match_identities = descendants.len() == descendant_identities.len()
            && descendants
                .iter()
                .zip(&descendant_identities)
                .all(|(signature, identity)| {
                    snapshot.entry(signature.pid).is_some_and(|entry| {
                        identity.creation_time() == entry.command().creation_time
                    })
                });
        if !descendants_match_identities
            || !shell_identity.running()
            || !selected_identity.running()
            || !descendant_identities.iter().all(ProcessIdentity::running)
        {
            return None;
        }
        let now = Instant::now();
        Some(Self {
            shell,
            selected,
            descendants,
            descendant_identities,
            shell_identity,
            selected_identity,
            job: job.clone(),
            verified_at: now,
            last_used: now,
        })
    }
}

impl ForegroundSelectionCache {
    fn get(&mut self, shell_pid: u32, snapshot: &ProcessSnapshot) -> Option<ForegroundJob> {
        if let Some(cached) = self.entries.get_mut(&shell_pid) {
            let current_descendants = descendant_entries(shell_pid, snapshot);
            let topology_matches = current_descendants.len() == cached.descendants.len()
                && cached
                    .descendants
                    .iter()
                    .all(|entry| entry.matches(snapshot.entry(entry.pid)));
            let valid = cached.verified_at.elapsed() < FOREGROUND_SELECTION_RECHECK
                && cached.shell_identity.running()
                && cached.selected_identity.running()
                && cached
                    .descendant_identities
                    .iter()
                    .all(ProcessIdentity::running)
                && cached.shell.matches(snapshot.entry(shell_pid))
                && cached
                    .selected
                    .matches(snapshot.entry(cached.job.process_group_id))
                && topology_matches;
            if valid {
                cached.last_used = Instant::now();
                return Some(cached.job.clone());
            }
        }
        self.entries.remove(&shell_pid);
        None
    }

    fn remember(&mut self, shell_pid: u32, cached: Option<CachedForegroundSelection>) {
        let Some(cached) = cached else {
            self.entries.remove(&shell_pid);
            return;
        };
        self.entries
            .retain(|_, cached| cached.last_used.elapsed() < FOREGROUND_SELECTION_CACHE_RETENTION);
        if self.entries.len() >= FOREGROUND_SELECTION_CACHE_CAPACITY {
            self.entries.clear();
        }
        self.entries.insert(shell_pid, cached);
    }

    #[cfg(test)]
    fn remember_for_test(
        &mut self,
        shell_pid: u32,
        snapshot: &ProcessSnapshot,
        job: &ForegroundJob,
    ) {
        let descendants = snapshot.descendant_signatures(shell_pid);
        let descendant_identities = descendants
            .iter()
            .map(|_| ProcessIdentity::Stub {
                running: true,
                creation_time: None,
            })
            .collect();
        let cached = CachedForegroundSelection::from_snapshot_with_identities(
            shell_pid,
            snapshot,
            job,
            descendants,
            descendant_identities,
            ProcessIdentity::Stub {
                running: true,
                creation_time: None,
            },
            ProcessIdentity::Stub {
                running: true,
                creation_time: None,
            },
        );
        self.remember(shell_pid, cached);
    }
}

impl ProcessSnapshotCache {
    fn snapshot(
        &mut self,
        max_age: Duration,
        build: impl FnOnce() -> Vec<WindowsProcessEntry>,
    ) -> Arc<ProcessSnapshot> {
        if let Some(cached) = &self.cached {
            if cached.built_at.elapsed() < max_age {
                return Arc::clone(&cached.snapshot);
            }
        }

        let snapshot = Arc::new(ProcessSnapshot::new(build()));
        self.cached = Some(CachedProcessSnapshot {
            built_at: Instant::now(),
            snapshot: Arc::clone(&snapshot),
        });
        snapshot
    }
}

fn read_process_command(pid: u32, name: &str) -> WindowsProcessCommand {
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

impl WindowsProcessCommand {
    fn from_cmdline(name: &str, creation_time: Option<u64>, cmdline: Option<String>) -> Self {
        let argv = cmdline.as_deref().and_then(command_line_to_argv);
        let argv0 = argv
            .as_ref()
            .and_then(|argv| argv.first().cloned())
            .or_else(|| (!name.is_empty()).then(|| name.to_string()));
        Self {
            creation_time,
            argv0,
            argv,
            cmdline,
        }
    }
}

fn process_is_git_bash(pid: u32) -> bool {
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

fn process_executable_path(process: HANDLE) -> Option<String> {
    let mut path = vec![0_u16; 32_768];
    let mut len = path.len() as u32;
    if unsafe { QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut len) } == 0 {
        return None;
    }
    String::from_utf16(&path[..len as usize]).ok()
}

fn command_uses_git_bash(command: &portable_pty::CommandBuilder) -> bool {
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

fn is_git_bash_executable_path(path: &std::path::Path) -> bool {
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

fn process_runtime_marker(pid: u32) -> Option<String> {
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

fn process_creation_time(process: HANDLE) -> Option<u64> {
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

fn process_runtime_marker_from_handle(process: HANDLE) -> Option<Option<String>> {
    let parameters = read_process_parameters(process)?;
    let environment = read_process_environment(process, parameters.environment)?;
    Some(environment_variable_from_utf16(
        &environment,
        PANE_RUNTIME_MARKER_ENV_VAR,
    ))
}

fn read_process_environment(process: HANDLE, address: *const c_void) -> Option<Vec<u16>> {
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

fn environment_variable_from_utf16(environment: &[u16], name: &str) -> Option<String> {
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

fn read_process_parameters(process: HANDLE) -> Option<RtlUserProcessParameters> {
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

fn command_line_to_argv(command_line: &str) -> Option<Vec<String>> {
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

fn nul_terminated_utf16_to_string(buffer: &[u16]) -> String {
    let len = buffer
        .iter()
        .position(|&value| value == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..len])
}

pub fn session_processes(child_pid: u32) -> Vec<u32> {
    if child_pid == 0 {
        return Vec::new();
    }

    let snapshot = ProcessSnapshot::new(snapshot_processes());
    session_processes_from_snapshot(child_pid, &snapshot)
}

fn session_processes_from_snapshot(child_pid: u32, snapshot: &ProcessSnapshot) -> Vec<u32> {
    if snapshot.entry(child_pid).is_none() {
        return Vec::new();
    }

    let mut pids = vec![child_pid];
    pids.extend(
        descendant_entries(child_pid, snapshot)
            .into_iter()
            .map(|entry| entry.pid),
    );
    pids
}

pub fn signal_processes(pids: &[u32], signal: Signal) {
    if signal == Signal::Hangup {
        return;
    }

    for &pid in pids {
        let Some(process) = ProcessHandle::open(pid, PROCESS_QUERY_LIMITED_INFORMATION) else {
            continue;
        };
        unsafe {
            TerminateProcess(process.0, 1);
        }
    }
}

pub fn process_exists(pid: u32) -> bool {
    let Some(process) = ProcessHandle::open(pid, PROCESS_QUERY_LIMITED_INFORMATION) else {
        return false;
    };

    let mut exit_code = 0;
    let ok = unsafe { GetExitCodeProcess(process.0, &mut exit_code) } != 0;
    ok && exit_code == STILL_ACTIVE
}

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

pub fn read_clipboard_image() -> Option<ClipboardImage> {
    for attempt in 0..10 {
        if unsafe { OpenClipboard(null_mut()) } != 0 {
            let _clipboard = ClipboardGuard;
            if let Some(bytes) = read_registered_png_clipboard() {
                return Some(ClipboardImage {
                    bytes,
                    extension: "png",
                });
            }
            for format in [CF_DIBV5 as u32, CF_DIB as u32] {
                if let Some(bytes) =
                    clipboard_global_bytes(format, clipboard_image::MAX_CLIPBOARD_ALLOCATION)
                {
                    if let Some(bytes) = clipboard_image::dib_to_png(&bytes) {
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

fn read_registered_png_clipboard() -> Option<Vec<u8>> {
    static PNG_FORMAT: LazyLock<u32> = LazyLock::new(|| {
        let name = wide_null("PNG");
        unsafe { RegisterClipboardFormatW(name.as_ptr()) }
    });
    if *PNG_FORMAT == 0 {
        return None;
    }
    let bytes = clipboard_global_bytes(
        *PNG_FORMAT,
        crate::protocol::MAX_CLIPBOARD_IMAGE_PAYLOAD + 64 * 1024,
    )?;
    clipboard_image::validated_png(&bytes)
}

fn clipboard_global_bytes(format: u32, max_bytes: usize) -> Option<Vec<u8>> {
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
        .name("herdr-windows-notification".into())
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

fn show_desktop_notification_on_thread(
    title: &str,
    body: &str,
    ready_tx: std::sync::mpsc::SyncSender<std::io::Result<bool>>,
) {
    let class_name = wide_null("STATIC");
    let window_name = wide_null("Herdr notifications");
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
    copy_wide_truncated(&mut notification.szTip, "Herdr");

    if unsafe { Shell_NotifyIconW(NIM_ADD, &notification) } == 0 {
        let _ = ready_tx.send(Err(std::io::Error::other(
            "failed to add Herdr notification-area icon",
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
            "failed to show Herdr desktop notification",
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

fn copy_wide_truncated<const N: usize>(destination: &mut [u16; N], value: &str) {
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

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

struct ProcessHandle(HANDLE);

struct ClipboardGuard;

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        unsafe {
            CloseClipboard();
        }
    }
}

impl ProcessHandle {
    fn open(pid: u32, access: u32) -> Option<Self> {
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
struct Peb {
    reserved1: [u8; 2],
    being_debugged: u8,
    reserved2: [u8; 1],
    reserved3: [*mut c_void; 2],
    ldr: *mut c_void,
    process_parameters: *mut RtlUserProcessParameters,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CurDir {
    dos_path: UNICODE_STRING,
    handle: HANDLE,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RtlUserProcessParameters {
    maximum_length: u32,
    length: u32,
    flags: u32,
    debug_flags: u32,
    console_handle: HANDLE,
    console_flags: u32,
    standard_input: HANDLE,
    standard_output: HANDLE,
    standard_error: HANDLE,
    current_directory: CurDir,
    dll_path: UNICODE_STRING,
    image_path_name: UNICODE_STRING,
    command_line: UNICODE_STRING,
    environment: *mut c_void,
}

fn read_process_value<T: Copy>(process: HANDLE, address: *const c_void) -> Option<T> {
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

fn read_unicode_string(process: HANDLE, unicode: UNICODE_STRING) -> Option<String> {
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

#[cfg(test)]
#[path = "tests/shell_test.rs"]
mod tests;
