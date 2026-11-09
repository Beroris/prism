; Prism's installer: a normal Windows setup wizard, for the current user, with no admin rights.
;
; Build it with scripts/package-windows.ps1, which passes the version and the built exe:
;   ISCC.exe /DAppVersion=1.0.0 /DSourceExe=..\target\release\prism.exe installer\prism.iss
;
; It installs where scripts/install-windows.ps1 always has, so either replaces the other.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceExe
  #define SourceExe "..\target\release\prism.exe"
#endif

#define AppName "Prism"
#define AppExe "prism.exe"

[Setup]
; Never change this: Windows knows the installed app by it, for upgrades and uninstalling.
AppId={{6F1C2B7E-4D3A-4E8B-9C21-5A7D0F3B8E64}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=Beroris
AppPublisherURL=https://github.com/Beroris/prism
AppSupportURL=https://github.com/Beroris/prism/issues
AppComments=A small, fast screenshot and annotation tool.
VersionInfoVersion={#AppVersion}
VersionInfoDescription={#AppName} Setup
; Per user: no admin prompt, and the same folder the install script uses.
PrivilegesRequired=lowest
DefaultDirName={localappdata}\Programs\{#AppName}
DisableDirPage=yes
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
DisableWelcomePage=no
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
SetupIconFile=..\assets\app.ico
WizardStyle=modern
WizardSizePercent=100
WizardSmallImageFile=..\assets\icon.png
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.17763
Compression=lzma2/max
SolidCompression=yes
OutputDir=..\dist
OutputBaseFilename=Prism-{#AppVersion}-Setup
; Prism is closed by the code below, so Windows' "close these apps" page isn't needed.
CloseApplications=no
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "autostart"; Description: "Start {#AppName} when I sign in"; GroupDescription: "Options:"
Name: "desktopicon"; Description: "Put a shortcut on the desktop"; GroupDescription: "Options:"; Flags: unchecked

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; DestName: "{#AppExe}"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"; Comment: "Screenshot and annotate"
; An update leaves the desktop as the user has it.
Name: "{userdesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Comment: "Screenshot and annotate"; Tasks: desktopicon; Check: not IsUpdate

[Registry]
; The same entry Prism's own "Start Prism when I sign in" switch writes, so the two agree.
; An update leaves it as Prism's own switch last set it.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "{#AppName}"; ValueData: """{app}\{#AppExe}"" --background"; Tasks: autostart; Flags: uninsdeletevalue; Check: not IsUpdate

[Run]
Filename: "{app}\{#AppExe}"; Description: "Start {#AppName} now"; Flags: postinstall nowait skipifsilent
; Prism started this update and quit for it, so it comes back when it's done, and says so.
Filename: "{app}\{#AppExe}"; Parameters: "--background --updated"; Flags: nowait; Check: IsUpdate

[UninstallDelete]
Type: dirifempty; Name: "{app}"

[Code]
const
  RunKey = 'Software\Microsoft\Windows\CurrentVersion\Run';

{ Prism runs Setup with /UPDATE when it updates itself. Setup then changes nothing but the
  app, and starts it again at the end. }
function IsUpdate(): Boolean;
var
  I: Integer;
begin
  Result := False;
  for I := 1 to ParamCount do
    if CompareText(ParamStr(I), '/UPDATE') = 0 then
      Result := True;
end;

{ Asks a running Prism to quit, so its exe can be replaced or removed and its hotkeys and
  keyboard hook let go. }
procedure QuitPrism();
var
  Code: Integer;
  I: Integer;
  Exe: String;
begin
  Exe := ExpandConstant('{app}\{#AppExe}');
  if FileExists(Exe) then
  begin
    Exec(Exe, '--quit', '', SW_HIDE, ewWaitUntilTerminated, Code);
  end;
  { Prism holds this until its process has gone, and with it its hold on the exe. }
  for I := 1 to 50 do
  begin
    if not CheckForMutexes('Local\Prism.Instance') then
      Break;
    Sleep(100);
  end;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  QuitPrism();
  Result := '';
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  { Unticking the sign-in option on an upgrade turns it off, as it says. }
  if (CurStep = ssPostInstall) and not IsUpdate() and not WizardIsTaskSelected('autostart') then
    RegDeleteValue(HKCU, RunKey, '{#AppName}');
end;

function SettingsFile(): String;
begin
  Result := ExpandConstant('{userappdata}\{#AppName}\settings.json');
end;

{ Whether Prism had taken Print Screen from Snipping Tool, so uninstalling gives it back. }
function TookPrintScreen(): Boolean;
var
  Text: AnsiString;
  Flat: String;
begin
  Result := False;
  if LoadStringFromFile(SettingsFile(), Text) then
  begin
    Flat := String(Text);
    StringChangeEx(Flat, ' ', '', True);
    Result := Pos('"replace_snipping":true', Flat) > 0;
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
  begin
    QuitPrism();
    if TookPrintScreen() then
      RegWriteDWordValue(HKCU, 'Control Panel\Keyboard', 'PrintScreenKeyForSnippingEnabled', 1);
    RegDeleteValue(HKCU, RunKey, '{#AppName}');
  end;
  if (CurUninstallStep = usPostUninstall) and DirExists(ExpandConstant('{userappdata}\{#AppName}')) and not UninstallSilent() then
  begin
    if MsgBox('Also remove your Prism settings?' #13#10#13#10 'Keep them if you might install Prism again.', mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
      DelTree(ExpandConstant('{userappdata}\{#AppName}'), True, True, True);
  end;
end;
