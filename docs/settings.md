# Settings reference

[Back to the README](../README.md) · [Building](building.md) · [How it works](how-it-works.md)

Settings live in `%LOCALAPPDATA%\SpeedMeter\config.json`. The window and the right-click menu edit this file for you; you can also edit it by hand while SpeedMeter is closed.

| Key | Default | Description |
| --- | --- | --- |
| `connection` | `{ "mode": "auto" }` | `auto`, `all`, or `{ "mode": "adapter", "id": "{GUID}", "name": "Ethernet" }` |
| `placement` | `taskbar_right` | `taskbar_right`, `taskbar_left` or `floating` |
| `offset` | `0` | Extra distance from the meter's anchor, in pixels at 100% scaling |
| `floating_pos` | `null` | Last position of the floating meter `[x, y]` |
| `layout` | `stacked` | `stacked` (two lines) or `inline` (one line) |
| `upload_first` | `true` | Which line comes first |
| `font` | `Bahnschrift` | Any installed font family; the window offers four that work well |
| `font_size` | `12.0` | 9 to 16 |
| `show_arrows` | `true` | Arrows next to the numbers |
| `animations` | `false` | Rolling numbers, growth and speed streaks |
| `unit` | `bytes` | `bytes` (KB/s, MB/s) or `bits` (Kbps, Mbps) |
| `decimals` | `auto` | `auto` (three digits), `one` or `two` |
| `heat` | on; 1, 10, 40 MB/s | `{ "enabled": true, "busy": 1048576, "heavy": 10485760, "extreme": 41943040 }` in bytes per second |
| `start_with_windows` | `true` | Adds or removes the sign-in entry |
| `show_tray_icon` | `true` | Notification-area icon |
| `theme` | `system` | Window colors: `system`, `dark` or `light` |
| `keep_minutes_days` | `30` | Days of minute-by-minute detail to keep |
| `auto_update` | `true` | Install new versions in the background |
| `limits` | all off | Data limits, see below |

### `limits`

| Key | Default | Description |
| --- | --- | --- |
| `daily`, `weekly`, `monthly` | off; 5, 25, 100 GB | `{ "on": true, "gb": 100 }`. GB here are 1024³ bytes, like everywhere in SpeedMeter. |
| `month_starts` | `1` | Day of the month (1 to 28) the monthly allowance starts again |
| `download_only` | `false` | Count only downloads toward the limits |
| `bar` | `true` | Show the usage bar under the meter's numbers (when a limit is on) |
| `bar_shows` | `closest` | `closest` (the limit nearest to running out), `daily`, `weekly` or `monthly` |
| `notify` | `true` | Windows notifications at 80%, 100% and every 25% over |

Per-app counting isn't a setting in this file: it's on when the counter service is installed (**Settings > Per-app usage**).
