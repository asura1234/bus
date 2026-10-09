use crate::platform::windows::shell::quote_windows_command_line_arg;
use std::cmp::Ordering;
use std::ffi::c_void;
use std::ffi::OsStr;
use std::mem::size_of;
use std::ptr::null_mut;
use windows_sys::Win32::Globalization::CompareStringOrdinal;
use windows_sys::Win32::Globalization::CSTR_EQUAL;
use windows_sys::Win32::Globalization::CSTR_GREATER_THAN;
use windows_sys::Win32::Globalization::CSTR_LESS_THAN;
use windows_sys::Win32::System::Console::GetConsoleWindow;
use windows_sys::Win32::System::JobObjects::IsProcessInJob;
use windows_sys::Win32::System::JobObjects::JobObjectExtendedLimitInformation;
use windows_sys::Win32::System::JobObjects::QueryInformationJobObject;
use windows_sys::Win32::System::JobObjects::JOBOBJECT_EXTENDED_LIMIT_INFORMATION;
use windows_sys::Win32::System::JobObjects::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
use windows_sys::Win32::System::Threading::GetCurrentProcess;
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
use windows_sys::Win32::System::Threading::DETACHED_PROCESS;

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

pub(in crate::platform::windows) fn launch_server_daemon_with_wmi(
    command: &std::process::Command,
) -> std::io::Result<u32> {
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

pub(in crate::platform::windows) fn windows_command_line(
    command: &std::process::Command,
) -> std::io::Result<String> {
    std::iter::once(command.get_program())
        .chain(command.get_args())
        .map(|value| {
            unicode_windows_value(value, "server command argument")
                .map(|value| quote_windows_command_line_arg(&value))
        })
        .collect::<std::io::Result<Vec<_>>>()
        .map(|parts| parts.join(" "))
}

pub(in crate::platform::windows) fn effective_command_environment(
    command: &std::process::Command,
) -> std::io::Result<Vec<String>> {
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

pub(in crate::platform::windows) fn windows_environment_key_cmp(
    left: &str,
    right: &str,
) -> Ordering {
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

pub(in crate::platform::windows) fn unicode_windows_value(
    value: &OsStr,
    label: &str,
) -> std::io::Result<String> {
    value.to_str().map(str::to_owned).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{label} is not valid Unicode"),
        )
    })
}

pub(in crate::platform::windows) fn current_process_is_in_job() -> std::io::Result<bool> {
    let mut in_job = 0;
    // SAFETY: `in_job` is a valid writable BOOL for the duration of the call.
    if unsafe { IsProcessInJob(GetCurrentProcess(), null_mut(), &mut in_job) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(in_job != 0)
}

pub(in crate::platform::windows) fn current_job_kills_processes_on_close() -> std::io::Result<bool>
{
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
