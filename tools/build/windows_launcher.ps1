param([Parameter(Mandatory)][string]$Executable, [switch]$Diagnostics)
$ErrorActionPreference = 'Stop'
$Executable = (Resolve-Path -LiteralPath $Executable).Path
$mt = Get-ChildItem "${env:ProgramFiles(x86)}/Windows Kits/10/bin/*/x64/mt.exe" |
    Sort-Object FullName -Descending | Select-Object -First 1
if (!$mt) { throw 'Windows SDK manifest tool (mt.exe) is required' }
$manifestFile = Join-Path ([IO.Path]::GetTempPath()) ([Guid]::NewGuid().ToString() + '.manifest')
try {
    & $mt.FullName "-inputresource:$Executable;#1" "-out:$manifestFile" -nologo
    if ($LASTEXITCODE) { throw "Cannot read launcher manifest: $LASTEXITCODE" }
    [xml]$manifest = Get-Content -LiteralPath $manifestFile -Raw -Encoding utf8
    $settings = $manifest.SelectSingleNode("//*[local-name()='windowsSettings']")
    if (!$settings) { throw 'Launcher manifest lacks Windows settings' }
    $codepage = $settings.SelectSingleNode("*[local-name()='activeCodePage']")
    if (!$codepage) {
        $codepage = $manifest.CreateElement('activeCodePage', 'http://schemas.microsoft.com/SMI/2019/WindowsSettings')
        [void]$settings.AppendChild($codepage)
    }
    # Confine UTF-8 to this process. No machine/user locale changes are needed.
    $codepage.InnerText = 'UTF-8'
    $manifest.Save($manifestFile)
    & $mt.FullName -manifest $manifestFile "-outputresource:$Executable;#1" -nologo
    if ($LASTEXITCODE) { throw "Cannot update launcher manifest: $LASTEXITCODE" }
} finally {
    Remove-Item -LiteralPath $manifestFile -ErrorAction SilentlyContinue
}
if ($Diagnostics) {
    $root = Split-Path -Parent $Executable
    $debugExe = Join-Path $root 'Paranoic-debug.exe'
    $image = [IO.File]::ReadAllBytes($Executable)
    $pe = [BitConverter]::ToInt32($image, 0x3c)
    if ([BitConverter]::ToUInt32($image, $pe) -ne 0x4550) { throw 'Invalid PE launcher' }
    $image[$pe + 24 + 68] = 3
    $image[$pe + 24 + 69] = 0
    [IO.File]::WriteAllBytes($debugExe, $image)
    Copy-Item (Join-Path $root 'app/Paranoic.cfg') (Join-Path $root 'app/Paranoic-debug.cfg') -Force
    $bat = @'
@echo off
cd /d "%~dp0"
set JPACKAGE_DEBUG=true
"%~dp0Paranoic-debug.exe" > "%~dp0startup-log.txt" 2>&1
set "PARANOIC_EXIT=%ERRORLEVEL%"
echo Exit code %PARANOIC_EXIT% >> "%~dp0startup-log.txt"
start notepad "%~dp0startup-log.txt"
exit /b %PARANOIC_EXIT%
'@
    [IO.File]::WriteAllText((Join-Path $root 'Paranoic-debug.bat'), ($bat -replace "`r?`n", "`r`n"), [Text.Encoding]::ASCII)
}
