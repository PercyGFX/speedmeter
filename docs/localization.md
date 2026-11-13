# Translating SpeedMeter

SpeedMeter keeps its original English text as the source and fallback language.
A Simplified Chinese (`zh-CN`) language pack is bundled into the main
`speedmeter.exe` using `include_str!`; the normal installer and updater do not
need to distribute language files separately.

## Changing the language

Open **Settings → Language**. The list is populated from the built-in English
and Chinese languages plus any valid additional language packs. Changing the
selection applies immediately and is saved in `config.json`.

If no language is explicitly selected yet, SpeedMeter uses the Windows UI
locale when there is an available pack, and English otherwise. For example,
`zh-CN` loads the built-in Chinese pack while `fr-FR` falls back to English
until a French pack is installed.

## Adding or editing a language without recompiling

Put a UTF-8 JSON file under:

```text
%LOCALAPPDATA%\SpeedMeter\locales\ja-JP.json
```

For tests or a portable copy using `SPEEDMETER_DATA_DIR`, place packs under
`%SPEEDMETER_DATA_DIR%\locales` instead.

Example:

```json
{
  "code": "ja-JP",
  "name": "日本語",
  "translations": {
    "Live": "リアルタイム",
    "History": "通信履歴",
    "Settings": "設定",
    "Language": "表示言語",
    "Network": "ネットワーク"
  }
}
```

- The file name must equal `code` (`ko-KR.json`, `fr-FR.json`, etc.).
- Keep every translation *key* in English exactly as used in the source.
- Preserve named placeholders such as `{amount}` or `{version}` in
  translated values.
- Missing or blank translations show the source English text.
- Invalid JSON, malformed language codes and files larger than 1 MiB are ignored.
- An external `zh-CN.json` overrides the built-in `zh-CN` pack. The
  built-in copy remains a fallback if the override is missing or malformed.
- Restart the dashboard to pick up newly added or edited files, without
  recompiling or reinstalling the program. Selection within the loaded list
  changes text immediately.

Translations are intentionally partial in this initial integration; new UI
strings should pass through `dashboard::i18n::tr` (static) or
`dashboard::i18n::tr_with` (messages with placeholders). For CJK glyph
rendering, the separate on-demand font fallback fix is tracked in PR #2.

This does not change networking, the ETW service, or the SQLite history schema.
