param([Parameter(Mandatory)][string]$Archive)
$ErrorActionPreference = 'Stop'
$report = Join-Path $PWD 'startup-results'
New-Item -ItemType Directory -Force $report | Out-Null
$unpacked = Join-Path $env:RUNNER_TEMP 'paranoic-startup-check'
Expand-Archive -LiteralPath $Archive -DestinationPath $unpacked -Force
$exe = Get-ChildItem $unpacked -Recurse -Filter Paranoic.exe | Select-Object -First 1
if (!$exe) { throw 'Paranoic.exe missing from portable archive' }
# Change only the subsystem field in this disposable copy. This gives the
# existing jpackage launcher a console without rebuilding the application.
$image = [IO.File]::ReadAllBytes($exe.FullName)
$pe = [BitConverter]::ToInt32($image, 0x3c)
if ([BitConverter]::ToUInt32($image, $pe) -ne 0x4550) { throw 'Invalid PE signature' }
$subsystem = $pe + 24 + 68
$image[$subsystem] = 3
$image[$subsystem + 1] = 0
[IO.File]::WriteAllBytes($exe.FullName, $image)
Add-Type @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class StartupWindow {
    delegate bool Callback(IntPtr window, IntPtr data);
    [DllImport("user32.dll")] static extern bool EnumWindows(Callback callback, IntPtr data);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr window, out uint pid);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetClassName(IntPtr window, StringBuilder text, int count);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetWindowText(IntPtr window, StringBuilder text, int count);
    public static bool Exists(int pid) {
        bool found = false;
        EnumWindows((window, data) => {
            uint owner; GetWindowThreadProcessId(window, out owner);
            if (owner != pid || !IsWindowVisible(window)) return true;
            var name = new StringBuilder(256); GetClassName(window, name, name.Capacity);
            var title = new StringBuilder(256); GetWindowText(window, title, title.Capacity);
            if (name.ToString() == "SunAwtFrame" && title.ToString() == "Paranoic") found = true;
            return true;
        }, IntPtr.Zero);
        return found;
    }
}
'@
$env:LOCALAPPDATA = Join-Path $unpacked 'user-data'
$env:JPACKAGE_DEBUG = 'true'
$started = Get-Date
$process = $null
try {
    $process = Start-Process $exe.FullName -WorkingDirectory $exe.DirectoryName -PassThru `
        -RedirectStandardOutput (Join-Path $report 'stdout.txt') -RedirectStandardError (Join-Path $report 'stderr.txt')
    $deadline = (Get-Date).AddSeconds(60)
    $visible = $false
    while ((Get-Date) -lt $deadline) {
        $process.Refresh()
        if ($process.HasExited) { throw "Portable exited before opening its window. Exit code $($process.ExitCode)" }
        if ([StartupWindow]::Exists($process.Id)) { $visible = $true; break }
        Start-Sleep -Milliseconds 500
    }
    if (!$visible) { throw 'Portable did not show its application window within 60 seconds' }
    Start-Sleep -Seconds 10
    $process.Refresh()
    if ($process.HasExited -or ![StartupWindow]::Exists($process.Id)) { throw 'Application window disappeared after startup' }
    'PASS: visible Paranoic AWT window survived startup.' | Tee-Object (Join-Path $report 'result.txt')
} catch {
    $_ | Out-String | Tee-Object (Join-Path $report 'result.txt')
    throw
} finally {
    if ($process -and !$process.HasExited) { Stop-Process -Id $process.Id -Force }
    Get-ChildItem $unpacked -Recurse -File | Where-Object {
        $_.Name -in @('startup-error.txt', 'last-jvm-crash-v1.bin') -or $_.Name -like 'hs_err_pid*.log'
    } | Copy-Item -Destination $report -Force
    Get-WinEvent -FilterHashtable @{LogName='Application'; StartTime=$started} -ErrorAction SilentlyContinue |
        Where-Object { $_.Message -match 'Paranoic|jvm.dll|skiko' } |
        Format-List TimeCreated,ProviderName,Id,Message | Out-File (Join-Path $report 'windows-events.txt')
    Get-Content (Join-Path $report 'stdout.txt'),(Join-Path $report 'stderr.txt') -ErrorAction SilentlyContinue
}
