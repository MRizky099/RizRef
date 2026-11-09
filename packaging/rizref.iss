; RizRef — Inno Setup 6 script for the optional per-user installer.
;
; Built by packaging/package.ps1, which passes the version and the staged
; directory:
;   ISCC.exe /DAppVersion=0.1.0 /DStageDir=dist\RizRef-0.1.0-x64 packaging\rizref.iss
;
; The installer is per-user (no administrator rights, like the portable zip):
; it copies the exe and the thumbnail DLL to %LOCALAPPDATA%\Programs\RizRef
; and lets the application register itself — `rizref.exe register` writes the
; same HKCU\Software\Classes keys a portable user gets by running it by hand,
; so there is one source of truth for the registry layout. Uninstalling runs
; `rizref.exe unregister` first.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef StageDir
  #error StageDir must point at the staged dist\RizRef-<version>-x64 directory
#endif

[Setup]
; A fixed AppId keeps upgrades in place. Never change it.
AppId={{B7C1E2A4-5D3F-4E8B-9A61-2F4C8D9E7A13}
AppName=RizRef
AppVersion={#AppVersion}
AppVerName=RizRef {#AppVersion}
AppPublisher=RizRef
AppPublisherURL=https://github.com/MRizky099/RizRef
DefaultDirName={localappdata}\Programs\RizRef
DefaultGroupName=RizRef
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputBaseFilename=RizRef-{#AppVersion}-x64-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\rizref.exe
LicenseFile={#StageDir}\LICENSE-MIT
ChangesAssociations=yes
MinVersion=10.0

[Tasks]
Name: "associate"; Description: "Open .rizref boards with RizRef and show their thumbnails in Explorer"; GroupDescription: "File association:"
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Additional shortcuts:"; Flags: unchecked

[Files]
Source: "{#StageDir}\rizref.exe"; DestDir: "{app}"; Flags: ignoreversion
; The thumbnail DLL may be loaded by Explorer during an upgrade; replace it on
; the next restart rather than failing the install.
Source: "{#StageDir}\rizref_thumb.dll"; DestDir: "{app}"; Flags: ignoreversion restartreplace uninsrestartdelete
Source: "{#StageDir}\README.txt"; DestDir: "{app}"; Flags: ignoreversion isreadme
Source: "{#StageDir}\LICENSE-MIT"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\LICENSE-APACHE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\RizRef"; Filename: "{app}\rizref.exe"
Name: "{autodesktop}\RizRef"; Filename: "{app}\rizref.exe"; Tasks: desktopicon

[Run]
; Per-user install: HKCU. An elevated per-machine install (chosen in the
; privileges dialog) registers for every user instead.
Filename: "{app}\rizref.exe"; Parameters: "register"; Flags: runhidden; Tasks: associate; Check: not IsAdminInstallMode
Filename: "{app}\rizref.exe"; Parameters: "register --machine"; Flags: runhidden; Tasks: associate; Check: IsAdminInstallMode
Filename: "{app}\rizref.exe"; Description: "Launch RizRef"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{app}\rizref.exe"; Parameters: "unregister"; Flags: runhidden; RunOnceId: "UnregisterUser"; Check: not IsAdminInstallMode
Filename: "{app}\rizref.exe"; Parameters: "unregister --machine"; Flags: runhidden; RunOnceId: "UnregisterMachine"; Check: IsAdminInstallMode
