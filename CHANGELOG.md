# Changelog

What's new in each version of SpeedMeter. The newest version is at the top.

## [0.3.0] - 2026-11-11

### New look

- **A new app icon:** a speed gauge that runs from blue to red.
- **Cleaner controls.** Selectors such as Day, Month and Year show the chosen option as a solid blue pill; switches, buttons and the back and forward buttons in History are redrawn to match.
- **A calmer calendar.** Days are shaded in one blue that deepens with use, and the busiest days stand out in red, like their bars in the chart.
- **Update card.** When a new version is ready, the sidebar shows a card with its progress instead of a plain button.
- Settings text now uses the full width of each row.

### Fixed

- In a small window, the History title no longer runs under the Day, Month and Year selector, and the Live legend no longer runs under the time-range selector.
- In the light theme, the selected page in the sidebar and the empty part of the limit bars are visible again.
- The content area's top-left corner is properly rounded.
- The live app list keeps showing while the per-app counter waits for an update, and its order no longer reshuffles every second.
- With a single data limit on, its bar on the Live page sits on the left instead of the far right.
- Per-app counting: downloads over UDP (video calls, many games, QUIC) are now matched to the right network. This matters when you measure one network while another, such as a VPN, is also in use. SpeedMeter asks to update the counter once (one administrator approval).

## [0.2.2] - 2026-10-27

### Changed

- **A calmer live app list.** "Apps using the network now" lists an app once it has used the network for about 3 seconds, and lets it go after 4 quiet seconds, so one-off blips no longer flash in and out. History still counts every byte.

### New

- **About.** Settings > About shows who made SpeedMeter and its license, with links to the project, the license and this changelog.
- SpeedMeter is now released under the [MIT License](https://github.com/PercyGFX/speedmeter/blob/main/LICENSE).

## [0.2.1] - 2026-10-27

### Fixed

- **Per-app counting showed no apps.** In 0.2.0 the counter didn't recognize Windows' network events, so it counted nothing. If you turned per-app counting on, SpeedMeter now asks you to update the counter (one administrator approval), and your apps appear within seconds.

### Changed

- The prompt to update the counter now also shows on the Live and History pages, not only in Settings.

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

[0.3.0]: https://github.com/PercyGFX/speedmeter/releases/tag/v0.3.0
[0.2.2]: https://github.com/PercyGFX/speedmeter/releases/tag/v0.2.2
[0.2.1]: https://github.com/PercyGFX/speedmeter/releases/tag/v0.2.1
[0.2.0]: https://github.com/PercyGFX/speedmeter/releases/tag/v0.2.0
[0.1.0]: https://github.com/PercyGFX/speedmeter/releases/tag/v0.1.0
