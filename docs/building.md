# Building SpeedMeter

[Back to the README](../README.md) · [How it works](how-it-works.md) · [Settings reference](settings.md)

## Requirements

- Windows 11 (or 10), 64-bit
- [Rust](https://rustup.rs) stable with the MSVC toolchain (`x86_64-pc-windows-msvc`)
- Visual Studio Build Tools with the Windows SDK (for the C compiler and `rc.exe`, used for the icon and manifest)
- Optional: [Inno Setup 6](https://jrsoftware.org/isinfo.php) to build the installer (`winget install JRSoftware.InnoSetup`)
- Optional: Python 3 with Pillow, only to regenerate the icon or sample data

## Build and run

```powershell
git clone https://github.com/PercyGFX/speedmeter.git
cd speedmeter
cargo run --release            # starts the meter and opens the window
cargo test                     # unit tests
.\installer\build.ps1          # tests, release build and installer -> installer\Output\
```

`cargo build --release` builds two executables: `speedmeter.exe` and the optional per-app counter `speedmeter-service.exe`. To try per-app counting from a build, use **Settings > Per-app usage > Turn on**. It installs the counter that sits next to `speedmeter.exe` and asks for administrator approval.

## Command-line flags

`speedmeter.exe`:

| Flag | Effect |
| --- | --- |
| *(none)* or `--open` | Start the meter if needed and open the window |
| `--background` | Start only the meter (used at sign-in) |
| `--dashboard` | Open only the window |
| `--page live\|history\|settings` | Which page the window opens on |
| `--quit` | Close every running SpeedMeter cleanly (used by the installer) |

`speedmeter-service.exe` (all need administrator rights):

| Flag | Effect |
| --- | --- |
| `--install` | Copy itself to `C:\Program Files\SpeedMeter`, register the service and start it. Also updates an older one. The app runs this for you. |
| `--uninstall` | Stop and remove the service and its files |
| `--run <seconds> <log file>` | Count in the foreground for a while and write a diagnostic log, for development |

## Environment variables

| Variable | Effect |
| --- | --- |
| `SPEEDMETER_DEMO=1` | The meter shows a synthetic sweep from a trickle to 80 MB/s instead of real traffic, spread over a few apps in the live list, and records nothing. Handy for trying the colors and animations. |
| `SPEEDMETER_DATA_DIR=<folder>` | Use another folder for `config.json`, `history.db` and `limits.json`. This copy runs on its own next to the normal one and never changes the sign-in entry. `python tools/make_demo_history.py <folder>` fills a folder with sample history, including per-app data. |

The README screenshots are taken from such a demo copy: sample history in its own folder, with `SPEEDMETER_DEMO=1`.

## Project layout

```
src/
  main.rs              entry point and command-line flags
  anim.rs              rolling numbers, growth spring, color crossfade
  apps.rs              per-app counting: reading the service, naming apps, setup
  appnet.rs            the per-app table shared by the service and the meter
  config.rs            settings (config.json)
  format.rs            KB/s -> MB/s -> GB/s formatting
  heat.rs              speed tiers and their colors
  history.rs           SQLite storage and queries
  limits.rs            data limits: periods, usage, notification steps
  net.rs               adapters, byte counters, automatic selection
  shared.rs            shared-memory ring buffer (meter -> window)
  sys.rs               Windows helpers: registry, theme, sign-in entry
  update.rs            GitHub release check, download, verify, install
  monitor/             the always-running meter
    mod.rs             one-second tick, minute buckets, menu, tray
    render.rs          Direct2D and DirectWrite drawing
    widget.rs          the meter window (in the taskbar or floating)
    taskbar.rs         finding the taskbar and the meter's slot
    layout.rs          UI Automation: where the taskbar buttons are
    tray.rs            notification-area icon and notifications
  dashboard/           the window
    mod.rs             app state, navigation, polling, updates
    live.rs            Live page
    history_view.rs    History page
    settings.rs        Settings page
    charts.rs          waveform, mirrored bars, calendar, table
    apps_ui.rs         app lists, live app speeds, per-app setup card
    icons.rs           app names and icons, loaded in the background
    theme.rs           colors, fonts, egui style
    ui.rs              Windows 11 style controls
  service/             speedmeter-service.exe, the optional per-app counter
    main.rs            entry point, the counting loop
    etw.rs             event tracing session (kernel network and process events)
    counters.rs        totals per process and adapter, the shared table
    scm.rs             running as a service, install and uninstall
assets/                icon, application manifest, resource script
installer/             Inno Setup script and local build script
tools/                 icon generator and sample-data generator
.github/workflows/     CI and the release pipeline
```

## Releasing a new version

1. Set the new `version` in `Cargo.toml` (for example `0.2.0`) and in `assets/app.rc` and `assets/app.manifest`, then build once so `Cargo.lock` updates.
2. Add a section for the version at the top of [CHANGELOG.md](../CHANGELOG.md). It becomes the release notes; the release stops if it's missing. Commit everything.
3. Tag and push:

   ```powershell
   git tag v0.2.0
   git push origin main v0.2.0
   ```

4. The [Release workflow](../.github/workflows/release.yml) runs the tests, builds both executables and the installer, and publishes `SpeedMeter-Setup-0.2.0.exe` on a new GitHub Release, with the changelog section as its notes. It stops if the tag and the `Cargo.toml` version don't match.
5. Installed copies pick it up within 12 hours (or right away from **Settings > Updates > Check for updates**).

If the release changes anything in `src/service`, bump `SERVICE_VERSION` in `src/appnet.rs`. Installed copies then offer to update the counter, which asks for administrator approval. Silent updates never touch the counter.

The repository has to stay public for installed copies to see new releases without a token.

**Code signing (optional).** To remove the SmartScreen warning, sign `speedmeter.exe`, `speedmeter-service.exe` and the installer before publishing, for example with Azure Trusted Signing or a code-signing certificate, by adding a signing step to the workflow after "Build" and after "Build installer".
