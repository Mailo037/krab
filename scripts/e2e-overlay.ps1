# End-to-end check of the real overlay on Windows, driven with synthetic input.
param([string]$Exe)

$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class Inp {
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
    [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
    [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint f, UIntPtr e);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    public struct POINT { public int X; public int Y; }
    public static uint ForegroundPid() { uint pid; GetWindowThreadProcessId(GetForegroundWindow(), out pid); return pid; }
    public static void Drag(int x0, int y0, int x1, int y1) {
        SetCursorPos(x0, y0); System.Threading.Thread.Sleep(60);
        mouse_event(0x0002, 0, 0, 0, UIntPtr.Zero); System.Threading.Thread.Sleep(40);
        for (int i = 1; i <= 12; i++) {
            SetCursorPos(x0 + (x1 - x0) * i / 12, y0 + (y1 - y0) * i / 12);
            System.Threading.Thread.Sleep(15);
        }
        mouse_event(0x0004, 0, 0, 0, UIntPtr.Zero);
    }
    public static void Key(byte vk) {
        keybd_event(vk, 0, 0, UIntPtr.Zero); System.Threading.Thread.Sleep(30);
        keybd_event(vk, 0, 2, UIntPtr.Zero);
    }
}
'@
[void][Inp]::SetProcessDPIAware()

$tmp = Join-Path $env:TEMP ("krab-e2e-" + [guid]::NewGuid().ToString("N").Substring(0, 8))
New-Item -ItemType Directory -Force $tmp | Out-Null
$saved = New-Object Inp+POINT
[void][Inp]::GetCursorPos([ref]$saved)

function Png-Size($path) {
    # Read width/height straight from the PNG IHDR chunk.
    $b = [IO.File]::ReadAllBytes($path)
    $w = ([int]$b[16] -shl 24) -bor ([int]$b[17] -shl 16) -bor ([int]$b[18] -shl 8) -bor [int]$b[19]
    $h = ([int]$b[20] -shl 24) -bor ([int]$b[21] -shl 16) -bor ([int]$b[22] -shl 8) -bor [int]$b[23]
    "${w}x${h}"
}

function Run-Case($name, $afterCapture, [scriptblock]$drive) {
    $cfg = Join-Path $tmp "$name.toml"
    $out = Join-Path $tmp "$name.png"
    "copy_to_clipboard = false`nauto_save = false`nnotifications = false`nafter_capture = `"$afterCapture`"" | Set-Content $cfg
    $env:KRAB_CONFIG = $cfg
    $p = Start-Process -FilePath $Exe -ArgumentList "--standalone", "-o", $out -PassThru
    # Only inject input once Krab's overlay really is the foreground window.
    $ready = $false
    foreach ($i in 1..40) {
        Start-Sleep -Milliseconds 50
        if ([Inp]::ForegroundPid() -eq $p.Id) { $ready = $true; break }
    }
    if (-not $ready) {
        if (-not $p.HasExited) { $p.Kill() }
        "{0,-22} FAIL overlay never became the foreground window" -f $name
        return
    }
    Start-Sleep -Milliseconds 150
    & $drive
    $exited = $p.WaitForExit(4000)
    if (-not $exited) { $p.Kill() }
    $file = if (Test-Path $out) { Png-Size $out } else { "none" }
    "{0,-22} exited={1} code={2} file={3}" -f $name, $exited, $p.ExitCode, $file
}

Run-Case "quick-copy" "copy" { [Inp]::Drag(300, 300, 700, 600) }
Run-Case "edit-then-enter" "edit" { [Inp]::Drag(300, 300, 620, 500); Start-Sleep -Milliseconds 200; [Inp]::Key(0x0D) }
Run-Case "edit-double-click" "edit" {
    [Inp]::Drag(400, 400, 600, 500); Start-Sleep -Milliseconds 200
    [Inp]::SetCursorPos(500, 450) | Out-Null
    foreach ($i in 1..2) {
        [Inp]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 30
        [Inp]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 60
    }
}
Run-Case "escape-cancels" "edit" { [Inp]::Key(0x1B) }
Run-Case "right-click-cancels" "edit" {
    [Inp]::SetCursorPos(500, 500) | Out-Null; Start-Sleep -Milliseconds 80
    [Inp]::mouse_event(8, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 30
    [Inp]::mouse_event(0x10, 0, 0, 0, [UIntPtr]::Zero)
}

[void][Inp]::SetCursorPos($saved.X, $saved.Y)
$env:KRAB_CONFIG = $null
Remove-Item -Recurse -Force $tmp
