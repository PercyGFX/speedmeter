; SpeedMeter installer (Inno Setup 6).
;
; Built by .github/workflows/release.yml, or locally with installer\build.ps1:
;   iscc /DAppVersion=0.2.0 installer\speedmeter.iss
;
; Installs per user into %LOCALAPPDATA%\Programs\SpeedMeter, so no admin prompt.
; The in-app updater runs this silently:
;   SpeedMeter-Setup-x.y.z.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /launch=--background
;
; The optional per-app counter (speedmeter-service.exe) is a Windows service, so
; setting it up asks for administrator approval. That only happens when the
; "Count data per app" task is ticked in an interactive install, never in a silent
; update; the app offers to update an older counter itself.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{6E1B6F1C-4A47-4C1B-9E8E-5D3C2A9B7F10}
AppName=SpeedMeter
AppVersion={#AppVersion}
AppVerName=SpeedMeter {#AppVersion}
AppPublisher=PercyGFX
AppCopyright=Copyright (c) 2026 PercyGFX. MIT License.
AppPublisherURL=https://github.com/PercyGFX/speedmeter
AppSupportURL=https://github.com/PercyGFX/speedmeter/issues
AppUpdatesURL=https://github.com/PercyGFX/speedmeter/releases
VersionInfoVersion={#AppVersion}
DefaultDirName={localappdata}\Programs\SpeedMeter
DisableDirPage=yes
DisableProgramGroupPage=yes
DisableReadyPage=yes
PrivilegesRequired=lowest
OutputDir=Output
OutputBaseFilename=SpeedMeter-Setup-{#AppVersion}
SetupIconFile=..\assets\app.ico
UninstallDisplayIcon={app}\speedmeter.exe
UninstallDisplayName=SpeedMeter
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Windows 10 1809 or newer (the taskbar meter is built for Windows 11).
MinVersion=10.0.17763
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "perapp"; Description: "Count data per app (asks for administrator approval)"; Check: not CounterInstalled

[Files]
Source: "..\target\release\speedmeter.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\target\release\speedmeter-service.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\SpeedMeter"; Filename: "{app}\speedmeter.exe"; Parameters: "--open"

[Run]
; Per-app counter: copies itself to Program Files and registers the service (elevated).
Filename: "{app}\speedmeter-service.exe"; Parameters: "--install"; Verb: "runas"; StatusMsg: "Setting up per-app counting..."; Flags: shellexec waituntilterminated runhidden; Tasks: perapp; Check: not WizardSilent
; Normal install: offer to start SpeedMeter (opens the window).
Filename: "{app}\speedmeter.exe"; Parameters: "--open"; Description: "Start SpeedMeter"; Flags: nowait postinstall skipifsilent
; Silent update: start it again the way the updater asked (--background or --open).
Filename: "{app}\speedmeter.exe"; Parameters: "{param:launch|--background}"; Flags: nowait; Check: WizardSilent

[UninstallRun]
Filename: "{app}\speedmeter.exe"; Parameters: "--quit"; Flags: runhidden waituntilterminated; RunOnceId: "QuitSpeedMeter"
; Removing the per-app counter service asks for administrator approval.
Filename: "{app}\speedmeter-service.exe"; Parameters: "--uninstall"; Verb: "runas"; Flags: shellexec waituntilterminated runhidden; RunOnceId: "RemoveCounter"; Check: CounterInstalled

[Registry]
; The app writes this value itself ("Start with Windows"); remove it on uninstall.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "SpeedMeter"; Flags: uninsdeletevalue dontcreatekey

[Code]
// The per-app counter service is registered (any user can read this key).
function CounterInstalled: Boolean;
begin
  Result := RegKeyExists(HKLM, 'SYSTEM\CurrentControlSet\Services\SpeedMeterApps');
end;

// Ask a running SpeedMeter to close before files are replaced. It saves the
// minute of history in progress on the way out.
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  ResultCode: Integer;
  Exe: String;
begin
  Exe := ExpandConstant('{app}\speedmeter.exe');
  if FileExists(Exe) then
    Exec(Exe, '--quit', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  Result := '';
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if (CurUninstallStep = usPostUninstall) and not UninstallSilent then
    if MsgBox('Also delete your traffic history and settings?', mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
      DelTree(ExpandConstant('{localappdata}\SpeedMeter'), True, True, True);
end;
