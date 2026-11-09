<img src="assets/icon.png" width="64" height="64" alt="">

# Prism

A small, fast screenshot and annotation tool for Windows. Press Print Screen, drag a region or click a window, mark it up right there, and press Enter. It works on its own: nothing else needs to be installed.

## What it does

- **Region, window or screen.** Drag to select, or click to take the window under the cursor. Resize with the handles, or nudge with the arrow keys (Ctrl+arrows resize). F takes the whole monitor and Ctrl+A every monitor.
- **Annotate in place.** Rectangle, ellipse, line, arrow, pen, highlighter, text, numbered steps, blur and pixelate. Eight colours with an optional fill, the scroll wheel for size, and Shift for straight lines and squares. Undo and redo, and the Select tool moves or deletes what you drew.
- **Copy text (OCR).** Ctrl+Print Screen, select, and the text is on the clipboard. Inside the editor, press O. This uses the OCR engine built into Windows, so there's nothing to download.
- **Magnifier and colour picker.** It shows a pixel-level zoom with coordinates and the colour under the cursor while you select. Press C to copy that colour as a hex value.
- **Copy, save or both.** Ctrl+C copies, Ctrl+S saves to your Screenshots folder, Ctrl+Shift+S asks where to save, and Enter does whichever one you choose in Settings. The clipboard gets a lossless PNG plus a standard bitmap.
- **Quiet confirmation.** A small card shows a thumbnail and fades away. Click it to show the file in Explorer.

## Keys

| Key | What it does |
|---|---|
| Ctrl+Alt+S, plus Print Screen and Win+Shift+S with Replace Snipping Tool on | Capture a region or a window |
| Shift+Print Screen | Capture the monitor under the cursor straight away |
| Ctrl+Print Screen | Copy text from a region |
| Enter / double-click | Copy, save or both: you choose in Settings, which can make Esc finish too |
| Hold Shift while letting go of a region | Finish straight away, without the toolbar. Choose the key, or turn it off, in Settings. |
| Ctrl+C / Ctrl+S / Ctrl+Shift+S | Copy / save / save as |
| V R E L A D H T N B X | Select, Rectangle, Ellipse, Line, Arrow, Draw, Highlighter, Text, Numbered step, Blur, Pixelate. Change the letters in Settings, under Shortcuts. |
| 1–8 | Pick a colour |
| Scroll wheel | Change the size |
| Ctrl+Z / Ctrl+Y | Undo / redo |
| Delete | Delete the selected shape |
| Right-click | Choose again before you draw; cancel while choosing |
| Esc | Finish typing, then close |

Change the shortcuts in Settings (right-click the tray icon, then Settings…): click one and press the keys you want.

**Replacing Snipping Tool.** Windows 11 gives Print Screen and Win+Shift+S to Snipping Tool. Turn on **Replace Snipping Tool** in Settings (the app also asks once on first run) and both shortcuts open Prism instead. Print Screen is switched with the setting Windows itself provides. Windows has no setting for Win+Shift+S, so the app catches that key with a keyboard hook, which is only installed while the option is on and keeps no record of what you type. Turn the option off and Snipping Tool gets both shortcuts back. Two limits: in a window running as administrator, Windows doesn't pass keys to the hook, so Snipping Tool still answers there; and if Print Screen keeps opening Snipping Tool, sign out and back in once.

## Install

Download **Prism-<version>-Setup.exe** from [Releases](https://github.com/Beroris/prism/releases/latest) and run it. It installs for you only, so it doesn't ask for admin rights, and adds Prism to the Start menu. Uninstall it from Settings → Apps like any other app; that also gives Print Screen back to Snipping Tool if Prism had it.

To run Prism without installing, download **Prism-<version>-Portable.exe** instead. Run it from anywhere, a USB stick included. It keeps its settings in a "Prism settings" folder beside itself rather than in your profile.

From a clone of this repo, these build and install it for you only:

```
pwsh scripts/install-windows.ps1              # installs, adds a Start menu entry, starts it
pwsh scripts/install-windows.ps1 -Uninstall   # removes it (add -Purge to delete settings too)
```

## Running it from other tools

Starting the exe again hands the command to the copy that's already running, which then opens the overlay in about 70 ms. That makes it easy to trigger from Orbit, a Stream Deck or a desktop shortcut:

```
prism.exe --region   # also the default with no argument
prism.exe --screen
prism.exe --text
prism.exe --quit
```

## Resource use

Measured on the release build on Windows 11 with a 2560×1440 monitor. See [docs/PERFORMANCE.md](docs/PERFORMANCE.md).

| | |
|---|---|
| Download | 1.1 MB single exe, no runtime needed |
| Idle memory | 1.8 MB private, 11 MB working set |
| Idle CPU | 0 ms over 10 s |
| Shortcut to overlay on screen | 50 ms, of which screen capture is 27 ms |
| Memory after a capture closes | back to 2.1 MB private |

## Build

You need Rust 1.85 or newer.

```sh
cargo build --release -p prism-windows        # target/release/prism.exe
cargo test --workspace
pwsh scripts/package-windows.ps1             # dist/: the installer and the portable exe (needs Inno Setup 6)
```

To regenerate the icon, run `cargo run -p prism-core --example make_ico`.

## Layout

| Path | What |
|---|---|
| `crates/prism-core` | Platform-neutral: annotation model with undo, rendering (tiny-skia), blur, pixelate, PNG, settings, file naming. No OS calls. |
| `platforms/windows` | The Windows app: Win32 + GDI. Tray, hotkeys, capture, overlay editor, clipboard, Windows OCR, toast. |
| `installer` | The setup wizard (Inno Setup), built by `scripts/package-windows.ps1`. |
| `assets/fonts` | Geist, the interface font, built into the exe (SIL Open Font License; see `Geist-LICENSE.txt`). |

A macOS host would reuse `prism-core` as is. See [docs/ROADMAP.md](docs/ROADMAP.md).

## Privacy

There's no network access, no account and no telemetry. Settings are in `%APPDATA%\Prism\settings.json`, and captures go only where you save or copy them.
