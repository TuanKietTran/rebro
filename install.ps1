# Build rebro (release) and install it to a stable place, so git's credential-helper path never moves.
#   pwsh -File install.ps1
# Installs: %LOCALAPPDATA%\Programs\rebro\{rebro,rebro-gui}.exe, user PATH, Start Menu "rebro".
param([switch]$NoGui)
$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    $env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"   # fresh rustup install, PATH not reloaded yet
}
# GPUI precompiles its shaders with fxc.exe (Windows SDK) in release builds. Without it, a debug
# build compiles them at runtime from the cargo registry copy of shaders.hlsl: works, but depends
# on that cache, so it's only a stop-gap until the SDK is installed.
function Find-Fxc {
    if ($env:GPUI_FXC_PATH -and (Test-Path $env:GPUI_FXC_PATH)) { return $env:GPUI_FXC_PATH }
    $c = Get-Command fxc.exe -ErrorAction SilentlyContinue
    if ($c) { return $c.Source }
    Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\fxc.exe" -ErrorAction SilentlyContinue |
        Sort-Object FullName -Descending | Select-Object -First 1 -ExpandProperty FullName
}

cargo build --release -p rebro
if ($LASTEXITCODE) { throw "cargo build (cli) failed" }
$guiExe = $null
if (-not $NoGui) {
    $fxc = Find-Fxc
    if ($fxc) {
        $env:GPUI_FXC_PATH = $fxc
        cargo build --release -p rebro-gui
        if ($LASTEXITCODE) { throw "cargo build (gui) failed" }
        $guiExe = 'target\release\rebro-gui.exe'
    } else {
        Write-Warning "fxc.exe (Windows SDK) not found: installing a debug-built GUI that compiles shaders at runtime from the cargo cache. For a self-contained GUI: winget install --id Microsoft.WindowsSDK.10.0.26100 -e, then rerun install.ps1"
        cargo build -p rebro-gui
        if ($LASTEXITCODE) { throw "cargo build (gui, debug) failed" }
        $guiExe = 'target\debug\rebro-gui.exe'
    }
}

$dest = Join-Path $env:LOCALAPPDATA 'Programs\rebro'
New-Item -ItemType Directory -Force $dest | Out-Null
# wait for it to exit: Stop-Process returns before Windows releases the exe's file lock
Get-Process rebro-gui -ErrorAction SilentlyContinue | Stop-Process -Force -PassThru | Wait-Process -Timeout 15 -ErrorAction SilentlyContinue
Copy-Item target\release\rebro.exe $dest -Force
if ($guiExe) { Copy-Item $guiExe (Join-Path $dest 'rebro-gui.exe') -Force; "gui: $guiExe" }

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (($userPath -split ';') -notcontains $dest) {
    [Environment]::SetEnvironmentVariable('Path', "$userPath;$dest", 'User')
    "added $dest to your user PATH (new terminals)"
}

if (-not $NoGui) {
    $lnk = Join-Path ([Environment]::GetFolderPath('Programs')) 'rebro.lnk'
    $sh = New-Object -ComObject WScript.Shell
    $s = $sh.CreateShortcut($lnk); $s.TargetPath = "$dest\rebro-gui.exe"; $s.WorkingDirectory = $dest
    $s.Description = 'forge accounts: GitHub, GitLab, Codeberg, Gitea'; $s.Save()
    "Start Menu: rebro"
}

# re-point git/gh at the installed binary (no-op when there are no accounts yet)
& "$dest\rebro.exe" sync
"installed to $dest"
