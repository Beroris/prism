# Performance

These figures come from one run on 2026-11-07, on the release build (`lto = "fat"`, static CRT). The machine was an AMD Ryzen 5 5600X with 16 GB of RAM, running Windows 11 Pro 10.0.26200 on a single 2560×1440 monitor at 100% scale. They describe this machine only, not a guarantee for others.

| Measurement | Result | How |
|---|---:|---|
| Exe size | 1.1 MB | `target/release/prism.exe` |
| Idle private memory | 1.8 MB | `Get-Process` 1.5 s after start, in the tray |
| Idle working set | 11–12 MB | same |
| Idle CPU | 0 ms in 10 s | `TotalProcessorTime` delta |
| Screen capture (BitBlt of the virtual desktop) | 25–27 ms | `PRISM_LOG` timing |
| Shortcut to overlay shown | 50–52 ms | same; includes the capture, the dimmed copy and the first frame |
| `--region` from a second process to overlay window | 72 ms | test harness, includes starting the forwarding process |
| Private memory while the overlay is open | 58.5 MB | about four full-screen buffers at 2560×1440, 14.7 MB each |
| Private memory after closing | 2.1 MB | 4.5 s after the save, once the toast had gone |
| OCR of 1280×720 of screen text | 260 ms | `--selftest-ocr`, debug build |
| Offscreen render of a full annotated frame | 60–90 ms | `--selftest-render`, debug build, cold start |

## Where the overlay's memory goes

The overlay holds four buffers the size of the virtual desktop: the capture, a dimmed copy, the capture with finished shapes, and the back buffer. All four are freed when it closes. On a 4K monitor that's about 130 MB for as long as the overlay is open.

If that becomes a problem, there are two easy cuts. The dimmed copy could be computed per frame, which costs about 2 ms a frame. The committed layer could cover only the selection.

## Not measured yet

- Two or more monitors, and monitors with different scale factors.
- HDR displays. GDI capture of HDR desktops is known to look washed out; Desktop Duplication would fix that.
- Low-end hardware.
- Real Print Screen key presses. The tests triggered the overlay with `--region` and drove it with posted window messages.
