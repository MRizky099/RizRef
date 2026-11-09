; RizRef — Inno Setup 6 script for the optional installer.
;
; Built by packaging/package.ps1, which passes the version and the staged
; directory:
;   ISCC.exe /DAppVersion=1.0.0 /DStageDir=dist\RizRef-1.0.0-x64 packaging\rizref.iss
;
; Per-user by default (no administrator rights, like the portable zip): the
; exe and the thumbnail DLL go to %LOCALAPPDATA%\Programs\RizRef. The
; privileges dialog also offers "install for all users", which elevates and
; defaults to Program Files. Either way the folder page always shows (also on
; upgrades, pre-filled with the previous folder), so the user can choose
; where RizRef lives; NextButtonClick checks the choice before any file is
; copied. A silent install takes the folder from /DIR="...".
;
; The application registers itself: `rizref.exe register` writes the path of
; the exe that runs it — the chosen {app} folder — into the open verb, the
; DefaultIcon and the thumbnail provider's InprocServer32, under
; HKCU\Software\Classes (per user) or, with --machine, HKLM (all users). It
; is the same command a portable user runs by hand, so there is one source of
; truth for the registry layout. Uninstalling runs `rizref.exe unregister`
; (with --machine for an all-users install) and removes the folder.

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
AppPublisher=Muhammad Rizky Abdillah (MRizky099)
AppPublisherURL=https://github.com/MRizky099/RizRef
; {autopf} is %LOCALAPPDATA%\Programs for a per-user install (unchanged
; from earlier builds) and Program Files for an all-users install.
DefaultDirName={autopf}\RizRef
; Always show the folder page, including on upgrades, starting from the
; folder the previous version was installed in.
DisableDirPage=no
UsePreviousAppDir=yes
DefaultGroupName=RizRef
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
; "dialog" offers "Install for all users"; "commandline" lets a silent install
; choose with /ALLUSERS or /CURRENTUSER.
PrivilegesRequiredOverridesAllowed=commandline dialog
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

[UninstallDelete]
; Files RizRef did not install (none today) would keep the folder; remove it
; once empty either way.
Type: dirifempty; Name: "{app}"

[UninstallRun]
Filename: "{app}\rizref.exe"; Parameters: "unregister"; Flags: runhidden; RunOnceId: "UnregisterUser"; Check: not IsAdminInstallMode
Filename: "{app}\rizref.exe"; Parameters: "unregister --machine"; Flags: runhidden; RunOnceId: "UnregisterMachine"; Check: IsAdminInstallMode

[Code]
const
  DRIVE_REMOTE = 4;

function GetDriveType(lpRootPathName: String): Cardinal;
  external 'GetDriveTypeW@kernel32.dll stdcall';

{ The deepest existing folder at or above Dir ('' if none). }
function ExistingAncestor(Dir: String): String;
var
  Parent: String;
begin
  Result := RemoveBackslashUnlessRoot(Dir);
  while (Result <> '') and not DirExists(Result) do
  begin
    Parent := ExtractFileDir(Result);
    if Parent = Result then
    begin
      Result := '';
      Exit;
    end;
    Result := Parent;
  end;
end;

{ Whether this process can create the folder and write into it, tested with
  a scratch folder in the deepest existing ancestor (removed again), so
  nothing is left behind if the user goes Back. }
function CanWriteTo(Dir: String): Boolean;
var
  Base, Probe: String;
begin
  Result := False;
  Base := ExistingAncestor(Dir);
  if Base = '' then
    Exit;
  Probe := AddBackslash(Base) + 'rizref-setup-probe-' + GetDateTimeString('hhnnsszzz', #0, #0);
  if not CreateDir(Probe) then
    Exit;
  Result := SaveStringToFile(AddBackslash(Probe) + 'probe.txt', 'RizRef', False);
  DeleteFile(AddBackslash(Probe) + 'probe.txt');
  RemoveDir(Probe);
end;

{ UNC paths and mapped network drives. }
function IsNetworkPath(Dir: String): Boolean;
var
  Drive: String;
begin
  if Copy(Dir, 1, 2) = '\\' then
  begin
    Result := True;
    Exit;
  end;
  Drive := ExtractFileDrive(Dir);
  Result := (Drive <> '') and (GetDriveType(AddBackslash(Drive)) = DRIVE_REMOTE);
end;

function NextButtonClick(CurPageID: Integer): Boolean;
var
  Dir: String;
begin
  Result := True;
  if CurPageID <> wpSelectDir then
    Exit;
  Dir := WizardDirValue;

  if not CanWriteTo(Dir) then
  begin
    if IsAdminInstallMode then
      SuppressibleMsgBox('Setup cannot write to:' + #13#10 + Dir + #13#10#13#10 +
        'Choose another folder.', mbError, MB_OK, IDOK)
    else
      SuppressibleMsgBox('Setup cannot write to:' + #13#10 + Dir + #13#10#13#10 +
        'Installing for this user only needs a folder you can write to without ' +
        'administrator rights, such as the suggested ' +
        ExpandConstant('{userpf}') + '\RizRef.' + #13#10#13#10 +
        'Choose another folder, or close Setup and start it again choosing ' +
        '"Install for all users" to install into Program Files.',
        mbError, MB_OK, IDOK);
    Result := False;
    Exit;
  end;

  if IsNetworkPath(Dir) then
    Result := SuppressibleMsgBox(Dir + #13#10#13#10 +
      'This is a network location. RizRef runs from there, but Windows ' +
      'Explorer usually refuses to load the thumbnail provider ' +
      '(rizref_thumb.dll) from a network folder, so .rizref files would show ' +
      'their icon instead of a preview.' + #13#10#13#10 +
      'Install here anyway?', mbConfirmation, MB_YESNO, IDYES) = IDYES;
end;
