; FileWisely Desktop — Inno Setup wrapper for install.ps1
; Requires: Inno Setup 6 (https://jrsoftware.org/isinfo.php)
;
; Before compiling:
;   1. Place Tauri release files in installer\uce\  (see uce\README.md)
;   2. Optional: place Bullzip (or PDF24) installer under installer\pdf-printer\
;   3. Set MyBusinessId below OR: ISCC.exe setup.iss /DMyBusinessId=xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx
;
; Compile: open this file in Inno Setup → Build → Compile
; CLI:     "C:\Program Files (x86)\Inno Setup 6\ISCC.exe" setup.iss

#define MyAppName "FileWisely"
#define MyAppVersion "1.0.0"
#define MyPublisher "FileWisely"
; Default tenant UUID for silent install — replace before shipping to a shop.
#define MyBusinessId "REPLACE_ME"

[Setup]
; Unique AppId — generate a new GUID for your product line if you fork this script.
AppId={{A8F3E2B1-4C5D-6E7F-8091-A2B3C4D5E6F7}}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyPublisher}
DefaultDirName=C:\FileWisely
DefaultGroupName={#MyAppName}
OutputBaseFilename=FileWisely-Setup
OutputDir=Output
Compression=lzma2/ultra64
SolidCompression=yes
PrivilegesRequired=admin
MinVersion=10.0
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64
; Fixed install root — no directory picker (matches install.ps1 paths).
DisableDirPage=yes
DisableProgramGroupPage=yes
; Minimal wizard (re-enable Finished page if you want a clear “done” screen for shops).
DisableReadyPage=yes
DisableFinishedPage=yes
WizardStyle=modern

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop icon for UCE"; GroupDescription: "Shortcuts:"; Flags: unchecked

[Files]
; Payload must live next to this .iss under installer\
Source: "install.ps1"; DestDir: "{app}"; Flags: ignoreversion
Source: "setup-filewisely-printer.ps1"; DestDir: "{app}"; Flags: ignoreversion
Source: "verify-filewisely-printer.ps1"; DestDir: "{app}"; Flags: ignoreversion
Source: "config.json.example"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist
Source: "uce\*"; DestDir: "{app}\uce"; Flags: recursesubdirs createallsubdirs ignoreversion
Source: "pdf-printer\*"; DestDir: "{app}\pdf-printer"; Flags: recursesubdirs createallsubdirs ignoreversion skipifsourcedoesntexist

[Run]
; Runs after files are installed to {app} (= C:\FileWisely). install.ps1 copies uce → C:\FileWisely\App and configures printer.
Filename: "{sys}\WindowsPowerShell\v1.0\powershell.exe"; \
  Parameters: "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File ""{app}\install.ps1"" -BusinessId ""{#MyBusinessId}"""; \
  StatusMsg: "Installing printer and UCE..."; \
  Flags: runhidden waituntilterminated

[Icons]
; Icons are created AFTER [Files] but BEFORE [Run]. install.ps1 copies uce → App\ later, so point at uce\UCE.exe (exists after extract). Tauri productName is ""UCE"".
Name: "{group}\FileWisely UCE"; Filename: "{app}\uce\UCE.exe"
Name: "{autodesktop}\FileWisely UCE"; Filename: "{app}\uce\UCE.exe"; Tasks: desktopicon
