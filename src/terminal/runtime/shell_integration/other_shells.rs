//! Editor witnesses for fish and PowerShell; neither handler accepts a line.
pub(super) const FISH: &str = r#"function __bus_capture_editor
    read -l __bus_challenge < __BUS_REQUEST__; or return
    begin
        printf '%s\n' "$__bus_challenge"
        commandline --current-buffer
    end > __BUS_CAPTURE_TMP__
    command /bin/mv -f __BUS_CAPTURE_TMP__ __BUS_CAPTURE__
end
function __bus_clear_editor
    commandline --replace ''
end
functions --copy fish_prompt __bus_original_prompt; or return
function fish_prompt
    __bus_original_prompt
    set -l __bus_status $status
    for __bus_map in default insert
        bind --mode $__bus_map \ezv __bus_capture_editor
        bind --mode $__bus_map \ezc __bus_clear_editor
    end
    printf '\e]133;A;bus=__BUS_PROMPT_TOKEN__\a'
    return $__bus_status
end
"#;

// No double quotes: portable-pty's powershell.exe argv round trip would alter
// them. File paths are PowerShell single-quoted literals, never evaluated code.
pub(super) const POWERSHELL: &str = r"Import-Module PSReadLine -ErrorAction Stop;
function global:__bus_capture_editor {
    $line = ''; $point = 0;
    [Microsoft.PowerShell.PSConsoleReadLine]::GetBufferState([ref]$line, [ref]$point);
    $challenge = [IO.File]::ReadAllText(__BUS_REQUEST__).TrimEnd([char]10);
    [IO.File]::WriteAllText(__BUS_CAPTURE_TMP__, $challenge + [char]10 + $line, [Text.UTF8Encoding]::new($false));
    [IO.File]::Delete(__BUS_CAPTURE__);
    [IO.File]::Move(__BUS_CAPTURE_TMP__, __BUS_CAPTURE__);
};
function global:__bus_clear_editor { [Microsoft.PowerShell.PSConsoleReadLine]::RevertLine() };
$global:__bus_original_prompt = $function:prompt;
function global:prompt {
    $out = @(& $global:__bus_original_prompt) -join ' ';
    Set-PSReadLineKeyHandler -Chord 'Alt+z,v' -ScriptBlock { __bus_capture_editor };
    Set-PSReadLineKeyHandler -Chord 'Alt+z,c' -ScriptBlock { __bus_clear_editor };
    $out + [char]27 + ']133;A;bus=__BUS_PROMPT_TOKEN__' + [char]7
}";
