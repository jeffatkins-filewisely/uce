; FileWisely — PDF printer only (Bullzip + silent global.ini + rename).
; Requires: Inno Setup 6 — https://jrsoftware.org/isinfo.php
;
; Place bullzip.exe under installer\pdf-printer\ before compiling (optional but recommended).
; Compile from the installer\ folder so relative Source paths resolve.
;
; CLI example:
;   "C:\Program Files (x86)\Inno Setup 6\ISCC.exe" FileWiselyInstaller.iss

#define MyAppName "FileWisely PDF Printer"
#define MyAppVersion "1.0.0"
#define MyPublisher "FileWisely"

[Setup]
AppId={{B2C4E6D8-1A3F-5E7C-9D0B-2E4F6A8C0D1E}}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyPublisher}
DefaultDirName=C:\FileWisely
DefaultGroupName={#MyAppName}
OutputBaseFilename=FileWiselyInstaller
OutputDir=Output
Compression=lzma2/ultra64
SolidCompression=yes
PrivilegesRequired=admin
MinVersion=10.0
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64
DisableDirPage=yes
DisableProgramGroupPage=yes
; Show the last wizard page so shops see a clear “done” message.
DisableFinishedPage=no
WizardStyle=modern

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Files]
; Same layout as repo installer\ (setup script resolves pdf-printer\bullzip.exe next to itself)
Source: "bullzip-silent-ini.ps1"; DestDir: "{app}"; Flags: ignoreversion
Source: "setup-filewisely-printer.ps1"; DestDir: "{app}"; Flags: ignoreversion
Source: "verify-filewisely-printer.ps1"; DestDir: "{app}"; Flags: ignoreversion
Source: "repair-bullzip-silent.ps1"; DestDir: "{app}"; Flags: ignoreversion
Source: "pdf-printer\*"; DestDir: "{app}\pdf-printer"; Flags: recursesubdirs createallsubdirs ignoreversion skipifsourcedoesntexist

[Run]
; Configure Bullzip for FileWisely (folder, global.ini, rename printer)
Filename: "{sys}\WindowsPowerShell\v1.0\powershell.exe"; \
  Parameters: "-NoProfile -ExecutionPolicy Bypass -File ""{app}\setup-filewisely-printer.ps1"""; \
  StatusMsg: "Configuring FileWisely PDF printer..."; \
  Flags: waituntilterminated

Filename: "{sys}\WindowsPowerShell\v1.0\powershell.exe"; \
  Parameters: "-NoProfile -ExecutionPolicy Bypass -File ""{app}\verify-filewisely-printer.ps1"""; \
  StatusMsg: "Verifying installation..."; \
  Flags: waituntilterminated

[Icons]
Name: "{group}\Verify FileWisely printer"; Filename: "{sys}\WindowsPowerShell\v1.0\powershell.exe"; \
  Parameters: "-NoProfile -ExecutionPolicy Bypass -File ""{app}\verify-filewisely-printer.ps1"""; WorkingDir: "{app}"
