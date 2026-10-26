# Changelog

What's new in each version of SpeedMeter. The newest version is at the top.

## [0.2.0] - 2026-10-27

### New

- **See which apps use your data.** The Live page lists the apps using the network right now, and History shows every app's download and upload for any day, month, year or single hour (click an hour in the day chart). Apps show with their own names and icons; Windows services appear under their own names, such as "Windows Update".
  - Optional. Turn it on in the installer or in **Settings > Per-app usage**. It adds a small background service, which asks for administrator approval once.
  - The service isn't a driver. It runs under Windows' limited Local Service account and never records addresses, websites or content, only how many bytes each app used.
- **Data limits.** Set a daily, weekly or monthly allowance in **Settings > Data limits**:
  - A thin bar under the numbers on the taskbar meter fills as you use it: blue, then amber from 80%, then red at 100%.
  - Windows notifications at 80%, at 100%, and every 25% over.
  - The monthly allowance can start on any day, like a billing date, and you can count downloads only.
  - Usage for each limit is also on the Live page and in the meter's tooltip.

### Changed

- Tab selectors (Day, Month, Year and the like) now show the chosen option as a raised tile instead of a small underline.
- Each page keeps its own scroll position, and the scroll bar no longer covers the page.
- The README is shorter; build instructions, technical details and the settings reference moved to [docs](https://github.com/PercyGFX/speedmeter/tree/main/docs).

### For developers

- A copy started with `SPEEDMETER_DATA_DIR` now runs completely on its own, next to the normal one, and never changes the sign-in entry. Demo mode (`SPEEDMETER_DEMO=1`) also fills the live app list.

## [0.1.0] - 2026-10-26

First release.

- Upload and download speed in the Windows 11 taskbar, next to the tray icons, after the app icons, or in a floating window.
- Speed colors, optional animations, light and dark taskbars.
- Live charts, and history by hour, day, month, year and all time with a calendar heatmap.
- Automatic network choice that follows Ethernet, Wi-Fi and VPNs.
- Installer without administrator rights, start with Windows, and automatic updates from GitHub Releases.

[0.2.0]: https://github.com/PercyGFX/speedmeter/releases/tag/v0.2.0
[0.1.0]: https://github.com/PercyGFX/speedmeter/releases/tag/v0.1.0
