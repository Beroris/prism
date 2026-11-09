# Roadmap

## v0.1 (this version, Windows)

Done and checked end to end on one machine. The check opened the overlay with `--region`, then drove it with posted mouse and key messages: select, arrow, numbered step, text, then Enter to save. The PNG and the toast came out right.

- Region, window and monitor capture, with a magnifier and colour picker
- In-place annotation: rectangle, ellipse, line, arrow, pen, highlighter, text, numbered steps, blur, pixelate, with undo, redo, move and delete
- Copy (PNG + DIB), save, save as, copy and save
- OCR text grab through Windows.Media.Ocr
- Tray menu settings, a JSON settings file, start with Windows
- A single instance, with command-line triggers for other tools

### Not tested yet

- Real Print Screen presses and the first-run question. Replacing Snipping Tool was tested: three rounds of an injected Win+Shift+S opened the overlay with keyboard focus, a real Esc closed it, and Snipping Tool never started. The Print Screen setting was restored afterwards.
- Copying to the clipboard, which was skipped so the clipboard wasn't overwritten during testing. The DIB layout has a unit test.
- The Save As and folder-picker dialogs, and Start with Windows.
- Several monitors, mixed DPI, and scale factors other than 100%.

## Next

- A small settings window, including a hotkey recorder, instead of editing JSON
- Pin a capture to the screen (always-on-top floating image)
- Repeat the last region
- Edit an existing image file in the same editor
- Desktop Duplication capture for HDR monitors
- A crop tool, an arrow style choice, and text with a background box
- Scrolling capture

## macOS

`prism-core` has no OS code and would carry over unchanged. A macOS host needs:

- capture through ScreenCaptureKit, which needs the Screen Recording permission
- an overlay NSWindow above everything, including the menu bar
- Vision for OCR
- NSPasteboard and a menu-bar item
