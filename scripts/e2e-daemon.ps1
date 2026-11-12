# End-to-end check of the background instance: hotkey, IPC hand-off, recording.
# Takes over the mouse and keyboard for a few seconds; the clipboard is not touched.
# Usage: scripts/e2e-daemon.ps1 -Exe target/release/krab.exe
param([string]$Exe)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class Inp3 {
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
    [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
    [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint f, UIntPtr e);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    public struct POINT { public int X; public int Y; }
    public static uint ForegroundPid() { uint pid; GetWindowThreadProcessId(GetForegroundWindow(), out pid); return pid; }
    static void S(int ms) { System.Threading.Thread.Sleep(ms); }
    public static void Drag(int x0, int y0, int x1, int y1) {
        SetCursorPos(x0, y0); S(60);
        mouse_event(0x0002, 0, 0, 0, UIntPtr.Zero); S(40);
        for (int i = 1; i <= 12; i++) { SetCursorPos(x0 + (x1 - x0) * i / 12, y0 + (y1 - y0) * i / 12); S(15); }
        mouse_event(0x0004, 0, 0, 0, UIntPtr.Zero);
    }
    public static void Click(int x, int y) {
        SetCursorPos(x, y); S(120);
        mouse_event(0x0002, 0, 0, 0, UIntPtr.Zero); S(40);
        mouse_event(0x0004, 0, 0, 0, UIntPtr.Zero);
    }
    public static void Key(byte vk) { keybd_event(vk, 0, 0, UIntPtr.Zero); S(30); keybd_event(vk, 0, 2, UIntPtr.Zero); }
    // Ctrl+Alt+Shift+F11
    public static void Hotkey() {
        byte[] mods = { 0x11, 0x12, 0x10 };
        foreach (var m in mods) keybd_event(m, 0, 0, UIntPtr.Zero);
        S(30); keybd_event(0x7A, 0, 0, UIntPtr.Zero); S(40); keybd_event(0x7A, 0, 2, UIntPtr.Zero);
        foreach (var m in mods) keybd_event(m, 0, 2, UIntPtr.Zero);
    }
}
'@
[void][Inp3]::SetProcessDPIAware()
$saved = New-Object Inp3+POINT
[void][Inp3]::GetCursorPos([ref]$saved)

$tmp = Join-Path $env:TEMP ("krab-e2e-" + [guid]::NewGuid().ToString("N").Substring(0, 8))
$shots = Join-Path $tmp "shots"
New-Item -ItemType Directory -Force $shots | Out-Null
$cfg = Join-Path $tmp "c.toml"
@"
hotkey = "Ctrl+Alt+Shift+F11"
hotkey_alt = ""
copy_to_clipboard = false
auto_save = true
save_dir = '$shots'
notifications = true
after_capture = "copy"
record_fps = 15
"@ | Set-Content $cfg
$env:KRAB_CONFIG = $cfg
$err = Join-Path $tmp "daemon.txt"

function Png-Size($path) {
    $b = [IO.File]::ReadAllBytes($path)
    $w = ([int]$b[16] -shl 24) -bor ([int]$b[17] -shl 16) -bor ([int]$b[18] -shl 8) -bor [int]$b[19]
    $h = ([int]$b[20] -shl 24) -bor ([int]$b[21] -shl 16) -bor ([int]$b[22] -shl 8) -bor [int]$b[23]
    "${w}x${h}"
}
function Wait-Foreground($procId, $ms) {
    foreach ($i in 1..($ms / 25)) { if ([Inp3]::ForegroundPid() -eq $procId) { return $true }; Start-Sleep -Milliseconds 25 }
    return $false
}
function New-Files($since) { Get-ChildItem $shots | Where-Object { $_.LastWriteTime -gt $since } }

$d = Start-Process -FilePath $Exe -ArgumentList "--daemon", "--quiet", "--trace" -PassThru -RedirectStandardError $err
Start-Sleep -Milliseconds 1200
"daemon running: $(-not $d.HasExited)"
$second = Start-Process -FilePath $Exe -ArgumentList "--daemon" -PassThru -Wait
"second --daemon exits immediately: code=$($second.ExitCode)"

# 1. Global hotkey -> overlay -> drag -> auto-saved file.
$t = Get-Date
[Inp3]::Hotkey()
if (Wait-Foreground $d.Id 2000) {
    Start-Sleep -Milliseconds 150
    [Inp3]::Drag(200, 200, 560, 440)
    Start-Sleep -Milliseconds 700
    $f = New-Files $t
    "hotkey snip: files=$($f.Count) size=$(if ($f) { Png-Size $f[0].FullName })"
} else { "hotkey snip: FAIL overlay did not open" }

# 2. `krab full --copy` handed to the daemon: no interaction needed.
Start-Sleep -Milliseconds 400
$t = Get-Date
$c = Start-Process -FilePath $Exe -ArgumentList "full", "--copy" -PassThru -Wait
Start-Sleep -Milliseconds 900
$f = New-Files $t
"ipc full snip: client code=$($c.ExitCode) files=$($f.Count) size=$(if ($f) { Png-Size $f[0].FullName })"

# 3. Recording: select, Enter, let it run, click Stop on the indicator.
Start-Sleep -Milliseconds 400
$t = Get-Date
$c = Start-Process -FilePath $Exe -ArgumentList "--record" -PassThru -Wait
if (Wait-Foreground $d.Id 2000) {
    Start-Sleep -Milliseconds 150
    [Inp3]::Drag(300, 300, 700, 600)
    Start-Sleep -Milliseconds 250
    [Inp3]::Key(0x0D)
    Start-Sleep -Milliseconds 2200
    $screenW = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds.Width
    [Inp3]::Click([int]($screenW / 2) + 50, 40)
    $rec = $null
    foreach ($i in 1..40) { Start-Sleep -Milliseconds 150; $rec = New-Files $t | Where-Object { $_.Extension -in ".mp4", ".gif" -and $_.Length -gt 0 }; if ($rec) { break } }
    Start-Sleep -Milliseconds 600
    "recording: file=$(if ($rec) { $rec[0].Extension + ' ' + $rec[0].Length + ' bytes' } else { 'none' })"
} else { "recording: FAIL overlay did not open" }

$q = Start-Process -FilePath $Exe -ArgumentList "--quit" -PassThru -Wait
[void]$d.WaitForExit(3000)
"daemon stopped by --quit: $($d.HasExited)"
"--- daemon log"
Get-Content $err
[void][Inp3]::SetCursorPos($saved.X, $saved.Y)
$env:KRAB_CONFIG = $null
Remove-Item -Recurse -Force $tmp
