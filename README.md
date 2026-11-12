<p align="center">
  <img src="assets/banner.png" alt="Krab - the snipping tool that opens in milliseconds" width="100%">
</p>

# Krab

**Krab** is a lightweight snipping tool that is on screen before you have finished pressing the key.
Select an area, mark it up, copy it. No splash screen, no GPU warm-up, no Electron: one small native binary
written in Rust.

The name is a nod to [Ferris](https://rustacean.net/), the Rust crab, plus *grab*.


## Why it is fast

| | Measured |
|---|---|
| Hotkey to overlay, Krab running in the background | **40 - 50 ms** |
| Cold start (`krab` with nothing running) to overlay | **about 70 ms** |
| Handing a request to the background instance | under 1 ms |
| Idle memory of the background instance | 14 MB working set, 2.4 MB private |
| Download | one 2.3 MB executable |

Measured on Windows 11 with a single 1920x1080 display. Most of the time is the screen grab itself.
Run `krab --trace` to see the numbers on your machine.

Krab draws everything itself on the CPU, grabs the screen while its window is still opening and keeps that
window ready in the background, so there is nothing to warm up.

## What it looks like

Select, then annotate in place:

![Annotating a snip](assets/screenshot-annotate.png)

Window snips highlight what you are about to capture:

![Window snip](assets/screenshot-window.png)

A pixel loupe for exact edges and colours:

![Pixel loupe](assets/screenshot-precision.png)

Screen recording and status feedback:

![Recording](assets/screenshot-record.png)
![Status surfaces](assets/screenshot-status.png)


## Features

Everything the Windows Snipping Tool does day to day, plus a few things it does not:

| | Krab |
|---|---|
| Rectangle, freeform, window and full-screen snips | yes |
| Delayed snips (3 / 5 / 10 s, or any value from the command line) | yes |
| Pen, highlighter, eraser | yes |
| Shapes: line, arrow, rectangle, ellipse | yes |
| Text | yes |
| Pixelate / redact | yes |
| Colour picker with pixel loupe (copies the hex value) | yes |
| Crop after selecting (move and resize handles, arrow keys) | yes |
| Undo / redo | yes |
| Copy to clipboard | yes |
| Save as PNG, JPEG, BMP or GIF | yes |
| Automatic save to `Pictures/Screenshots` | yes |
| Text extraction (OCR) and QR codes | yes - see notes below |
| Screen recording | yes, MP4 with ffmpeg or GIF without - no audio |
| Open the snip in another app | yes (system default app) |
| Global hotkey, tray icon, start with the system | yes (Windows, macOS) |
| Optional border around every snip | yes |
| Multiple monitors | yes, one overlay per monitor (one selection at a time) |
| Command line and scripting | yes |
| Automatic updates | yes |

Not included: audio in recordings, the share sheet, printing, and HDR colour correction.

## Install

Download the archive for your system from the
[releases page](https://github.com/Mailo037/krab/releases), unpack it and run `krab`.

| System | File |
|---|---|
| Windows 10 / 11 (x64) | `krab-windows-x64.zip` |
| macOS 11+ (Apple silicon and Intel) | `krab-macos-universal.tar.gz` |
| Linux (x86_64, X11 or Wayland, glibc 2.39+ such as Ubuntu 24.04) | `krab-linux-x86_64.tar.gz` |

Krab keeps itself up to date - see [Updates](#updates).

### Platform notes

**Windows** - start `krab --daemon` once (or enable *Start with system* in the tray menu). Krab then lives in the
tray and listens for `PrintScreen` and `Alt+Shift+S`. If Windows itself uses `PrintScreen` for its own snipping
tool, turn that off under *Settings > Accessibility > Keyboard*, or just use `Alt+Shift+S`.

**macOS** - the first capture asks for the *Screen Recording* permission (System Settings > Privacy & Security).
The binary is not signed; remove the quarantine flag after downloading: `xattr -d com.apple.quarantine krab`.
Default hotkey: `Cmd+Shift+2`. After an update macOS may ask for the permission once more.

**Linux** - bind the `krab` command to a key in your desktop's keyboard settings; that works on both X11 and
Wayland (Wayland does not allow applications to register global hotkeys themselves). Running `krab --daemon`
in the background makes those launches faster. Screen capture on Wayland uses PipeWire 1.0 or newer.

**Text extraction** uses the OCR engine built into Windows. On macOS and Linux it calls
[`tesseract`](https://github.com/tesseract-ocr/tesseract) if it is installed. QR codes are decoded by Krab itself
on every platform.

**Recording** writes H.264 MP4 when `ffmpeg` is on your `PATH`, and animated GIF otherwise.

## Updates

Krab updates itself. Once a day it looks for a new version on the releases page, downloads it, checks it
against the published checksums and replaces itself - a running background instance restarts into the new
version as soon as you are not in the middle of a snip or a recording, and tells you when it is done.

- Check right now: *Check for updates* in the tray menu, or `krab --update`.
- Turn the automatic check off: `auto_update = false` in the configuration.
- Updates need `curl`, which comes with Windows 10 / 11 and macOS and is installed on practically every Linux
  desktop, and write access to the folder Krab is in.

## Usage

```text
krab [MODE] [OPTIONS]      take a snip
krab --daemon              run in the background (tray icon + global hotkey)

MODES:
    rect          drag a rectangle (default)
    window        click a window
    full          the whole screen
    freeform      draw any shape

OPTIONS:
    -d, --delay <SECONDS>      wait before capturing (menus, hover states...)
    -r, --record               record the screen instead of taking a picture
    -c, --copy                 copy as soon as the area is selected
    -e, --edit                 always open the markup tools after selecting
    -o, --output <FILE>        also save the result to FILE (png, jpg, bmp, gif)
        --now                  capture the full screen right now, no overlay
        --standalone           do not reuse the background instance

BACKGROUND INSTANCE:
        --daemon               start it
        --quit                 stop it
        --autostart <on|off>   start Krab when you log in
        --config               open the configuration file
        --update               install the newest version now
```

Examples:

```sh
krab                      # drag an area, annotate, press Enter to copy
krab window --copy        # click a window, it is on the clipboard immediately
krab full --now -o a.png  # scripted full-screen capture, no UI at all
krab --delay 5            # five seconds to open that menu first
krab --record             # choose an area and record it
```

### Keyboard and mouse

| While selecting | |
|---|---|
| `R` `F` `W` `M` | rectangle, freeform, window, full screen (monitor) |
| `Esc` or right click | cancel |

| After selecting | |
|---|---|
| `Enter`, `Ctrl+C` or double click | copy and close |
| `Ctrl+S` | save as... |
| `Ctrl+Z` / `Ctrl+Y` | undo / redo |
| `Ctrl+A` | select the whole screen |
| `V` `P` `H` `L` `A` `R` `E` `T` `B` `X` `I` | move, pen, highlighter, line, arrow, rectangle, ellipse, text, pixelate, eraser, colour picker |
| arrow keys (`Shift` = 10 px, `Ctrl` = resize) | nudge the selection |
| `Shift` while drawing | straight strokes, 45 degree lines, squares and circles |
| mouse wheel | stroke size |
| `Esc` or right click | step back (text, then selection, then close) |

On macOS use `Cmd` instead of `Ctrl`.

### Configuration

`krab --config` opens `config.toml` (created with defaults on first use). Set `KRAB_CONFIG` to use a different file.

```toml
hotkey = "PrintScreen"
hotkey_alt = "Alt+Shift+S"
default_mode = "rect"          # rect | freeform | window | full
after_capture = "edit"         # "edit" = annotate in place, "copy" = copy immediately
copy_to_clipboard = true
auto_save = true
save_dir = ""                  # empty = Pictures/Screenshots
file_format = "png"            # png | jpg | bmp | gif
jpeg_quality = 92
notifications = true
border = false
border_color = "#F46623"
border_width = 2
loupe = true
delay_seconds = 3
record_fps = 30
color = "#FF3B30"
auto_update = true             # install new versions automatically
```

## License

[MIT](LICENSE). The bundled [Inter](https://rsms.me/inter/) typeface is licensed under the
[SIL Open Font License](assets/fonts/LICENSE.txt).
