#ifndef SourceExe
  #error SourceExe must be provided with /DSourceExe=...
#endif
#ifndef OutputDir
  #define OutputDir "dist/windows"
#endif
#ifndef Architecture
  #define Architecture "x64"
#endif
#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif

#define AppName "Harbor Light"
#define AppExeName "HarborLight.exe"

[Setup]
AppId={{B5DC2168-6849-4C41-B913-77FA313676AA}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher=Harbor Light
DefaultDirName={localappdata}\HarborLight
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
OutputDir={#OutputDir}
OutputBaseFilename=HarborLight-{#AppVersion}-windows-{#Architecture}-setup
SetupIconFile=..\..\resources\AppIcon.ico
UninstallDisplayIcon={app}\{#AppExeName}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no
ChangesAssociations=no

#if Architecture == "arm64"
ArchitecturesAllowed=arm64
ArchitecturesInstallIn64BitMode=arm64
#else
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
#endif

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; DestName: "{#AppExeName}"; Flags: ignoreversion

[Icons]
Name: "{group}\Harbor Light"; Filename: "{app}\{#AppExeName}"
Name: "{userdesktop}\Harbor Light"; Filename: "{app}\{#AppExeName}"; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "创建桌面快捷方式"; GroupDescription: "其他选项："; Flags: unchecked

[Run]
Filename: "{app}\{#AppExeName}"; Parameters: "install --dest ""{app}\{#AppExeName}"" --skip-bundle --skip-launch"; StatusMsg: "正在配置 Hooks 和登录自启动..."; Flags: runhidden waituntilterminated
Filename: "{app}\{#AppExeName}"; Description: "启动 Harbor Light"; Flags: nowait skipifsilent

[UninstallRun]
Filename: "{app}\{#AppExeName}"; Parameters: "uninstall --dest ""{app}\{#AppExeName}"""; RunOnceId: "HarborLightCleanup"; Flags: runhidden waituntilterminated skipifdoesntexist
