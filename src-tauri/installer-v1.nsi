Unicode true
; Set the compression algorithm. Default is LZMA.
!if "{{compression}}" == ""
  SetCompressor /SOLID lzma
!else
  SetCompressor /SOLID "{{compression}}"
!endif

!include MUI2.nsh
!include FileFunc.nsh
!include x64.nsh
!include WordFunc.nsh
!include "StrFunc.nsh"
!include "Win\COM.nsh"
!include "Win\Propkey.nsh"

; NSIS installer hooks for p2premote-service
; perMachine install mode runs as admin, no UAC needed

; A manually launched upgrade uses the same visible wizard and component
; progress as a fresh install. Service- or app-triggered upgrades can continue
; to pass /S or /P and remain fully unattended.

!macro NSIS_HOOK_PREINSTALL
  nsExec::ExecToLog 'cmd /c echo PREINSTALL >> C:\Windows\Temp\p2premote-installer-trace.log'
  ; Stop existing processes before overwriting installed files.
  DetailPrint "Stopping existing p2pRemote processes..."

  IfFileExists "$INSTDIR\resources\p2premote-service.exe" 0 +3
    ExecWait '"$INSTDIR\resources\p2premote-service.exe" --scm stop' $0
    DetailPrint "p2premote-service stop exit code: $0"

  ; Fallback cleanup. Missing processes are expected during first install.
  nsExec::ExecToLog 'taskkill /F /IM p2premote.exe /T'
  nsExec::ExecToLog 'taskkill /F /IM p2premote-service.exe /T'
  nsExec::ExecToLog 'taskkill /F /IM p2premote-notifier.exe /T'
!macroend

!macro NSIS_HOOK_POSTINSTALL
  nsExec::ExecToLog 'cmd /c echo POSTINSTALL_BEGIN >> C:\Windows\Temp\p2premote-installer-trace.log'
  ; Allow authenticated WGVPN peers to reach the persistent TCP health endpoint.
  ; Restrict the rule to the RFC 6598 virtual network instead of exposing the port globally.
  nsExec::ExecToLog 'netsh advfirewall firewall delete rule name="p2pRemote WGVPN Health"'
  nsExec::ExecToLog 'netsh advfirewall firewall add rule name="p2pRemote WGVPN Health" dir=in action=allow protocol=TCP localport=48082 remoteip=100.64.0.0/10 program="$INSTDIR\resources\p2premote-service.exe" profile=any enable=yes'
  nsExec::ExecToLog 'netsh advfirewall firewall delete rule name="p2pRemote WGVPN Speed Test"'
  nsExec::ExecToLog 'netsh advfirewall firewall add rule name="p2pRemote WGVPN Speed Test" dir=in action=allow protocol=UDP localport=48082 remoteip=100.64.0.0/10 program="$INSTDIR\resources\p2premote-service.exe" profile=any enable=yes'
  ; Probe the persisted auto_start flag before touching the service.
  ; The uninstall phase of a reinstall deletes the HKCU Run autostart entries
  ; and the service itself, while data\config.json survives by default and only
  ; stores non-default values (the key is present only when the user enabled it).
  StrCpy $1 "unknown"
  IfFileExists "$INSTDIR\data\config.json" 0 autorun_probed
  ; Note: the trailing space is required 鈥?NSIS does not expand a $\" escape
  ; that directly precedes the closing quote of the string.
  nsExec::ExecToLog 'findstr /C:$\"\$\"auto_start\$\": true$\" $\"$INSTDIR\data\config.json$\" '
  Pop $0
  ${If} $0 = 0
    StrCpy $1 "on"
  ${Else}
    StrCpy $1 "off"
  ${EndIf}
  autorun_probed:

  ; Restore the HKCU Run autostart entries removed by the uninstall phase
  ; (Tauri's template deletes the app entry, PREUNINSTALL deletes the notifier's).
  ${If} $1 == "on"
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "p2premote" '"$INSTDIR\p2premote.exe"'
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "p2premote-notifier" '"$INSTDIR\resources\p2premote-notifier.exe" --agent'
    DetailPrint "Restored autostart registry entries (auto_start enabled)"
  ${EndIf}

  ; Install + enable(AutoStart) + start the service. Retry to survive the SCM
  ; marked-for-delete window that the uninstall phase can leave behind; a failed
  ; setup would otherwise leave the machine without the service after a reboot.
  StrCpy $2 0
  scm_setup_retry:
  ExecWait '"$INSTDIR\resources\p2premote-service.exe" --scm setup "$INSTDIR\resources\p2premote-service.exe"' $0
    nsExec::ExecToLog 'cmd /c echo SETUP_RC_$0 >> C:\Windows\Temp\p2premote-installer-trace.log'
    ${If} $0 = 0
      Goto scm_setup_done
    ${EndIf}
    IntOp $2 $2 + 1
    ${If} $2 < 3
      Sleep 2000
      Goto scm_setup_retry
    ${EndIf}
  scm_setup_done:
  DetailPrint "p2premote-service setup exit code: $0 (attempts: $2)"
  ${If} $0 != 0
    Abort "p2pRemote services could not be installed"
  ${EndIf}

  ; A service can still be left stopped when Windows 7 reports a transient SCM
  ; state during an in-place upgrade, even though the setup helper returned 0.
  ; Make the post-install invariant explicit: a successful installation leaves
  ; the service running.  Starting an already-running service only returns the
  ; normal SCM "already running" error and is intentionally ignored here.
  ExecWait '"$INSTDIR\resources\p2premote-service.exe" --scm start' $4
  nsExec::ExecToLog 'cmd /c echo FINAL_START_RC_$4 >> C:\Windows\Temp\p2premote-installer-trace.log'
  DetailPrint "p2premote-service final start exit code: $4"

  ; Respect a persisted auto_start=off: setup force-enables AutoStart, so drop
  ; the boot autostart again 鈥?a reinstall must not silently revert the choice.
  ${If} $1 == "off"
    ExecWait '"$INSTDIR\resources\p2premote-service.exe" --scm disable' $0
    DetailPrint "p2premote-service disable exit code: $0"
  ${EndIf}

  ; Restore remote connectivity before starting the slower optional desktop
  ; engine install. The RustDeskTiny installer is idempotent and unattended, so
  ; leaving the outer GUI open cannot leave p2pRemote permanently offline.
  DetailPrint "Installing or upgrading RustDeskTiny..."
  ExecWait '"$INSTDIR\resources\RustDeskTiny-install.exe" --silent-install --install-dir "$INSTDIR\resources\RustDeskTiny"' $3
  DetailPrint "RustDeskTiny installer exit code: $3"
  ${If} $3 != 0
    ; RustDeskTiny is optional. On Win7 its installer may return a non-zero
    ; compatibility code; keep the primary p2pRemote service running.
    DetailPrint "Warning: RustDeskTiny installation failed; p2pRemote remains available"
    nsExec::ExecToLog 'cmd /c echo RUSTDESKTINY_RC_$3 >> C:\Windows\Temp\p2premote-installer-trace.log'
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::ExecToLog 'netsh advfirewall firewall delete rule name="p2pRemote WGVPN Health"'
  nsExec::ExecToLog 'netsh advfirewall firewall delete rule name="p2pRemote WGVPN Speed Test"'

  ; Stop and uninstall the service before removing files
  ExecWait '"$INSTDIR\resources\p2premote-service.exe" --scm stop' $0
  ExecWait '"$INSTDIR\resources\p2premote-service.exe" --scm uninstall' $0

  ; Make sure no process keeps installed files locked.
  nsExec::ExecToLog 'taskkill /F /IM p2premote.exe /T'
  nsExec::ExecToLog 'taskkill /F /IM p2pRemote.exe /T'
  nsExec::ExecToLog 'taskkill /F /IM p2premote-service.exe /T'
  nsExec::ExecToLog 'taskkill /F /IM p2premote-cli.exe /T'
  nsExec::ExecToLog 'taskkill /F /IM p2premote-notifier.exe /T'

  ; Explicit cleanup for bundled helper binaries.
  Delete "$INSTDIR\p2premote.exe"
  Delete "$INSTDIR\p2pRemote.exe"
  Delete "$INSTDIR\resources\p2premote-service.exe"
  Delete "$INSTDIR\resources\p2premote-cli.exe"
  Delete "$INSTDIR\resources\p2premote-notifier.exe"
  Delete "$INSTDIR\resources\p2premote-desktop-engine.exe"
  Delete "$INSTDIR\resources\RustDeskTiny-install.exe"
  Delete "$INSTDIR\resources\wintun.dll"
  ; Intentionally non-recursive: RustDeskTiny owns its service and installed files.
  ; Keeping resources\RustDeskTiny prevents leaving a service with a missing binary.
  RMDir "$INSTDIR\resources"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "p2premote-notifier"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; p2pRemote stores machine-wide runtime data under the per-machine install root,
  ; while Tauri's built-in checkbox only removes APPDATA/LOCALAPPDATA paths.
  ; Respect the same checkbox and never delete data during an in-place update.
  ${If} $DeleteAppDataCheckboxState = 1
    DetailPrint "Removing p2pRemote application data..."
    RMDir /r /REBOOTOK "$INSTDIR\data"
    RMDir /REBOOTOK "$INSTDIR"
  ${EndIf}
!macroend
${StrCase}
${StrLoc}

!define MANUFACTURER "{{manufacturer}}"
!define PRODUCTNAME "{{product_name}}"
!define VERSION "{{version}}"
!define VERSIONWITHBUILD "{{version_with_build}}"
!define INSTALLMODE "{{install_mode}}"
!define LICENSE "{{license}}"
!define INSTALLERICON "{{installer_icon}}"
!define SIDEBARIMAGE "{{sidebar_image}}"
!define HEADERIMAGE "{{header_image}}"
!define MAINBINARYNAME "{{main_binary_name}}"
!define MAINBINARYSRCPATH "{{main_binary_path}}"
!define BUNDLEID "{{bundle_id}}"
!define COPYRIGHT "{{copyright}}"
!define OUTFILE "{{out_file}}"
!define ARCH "{{arch}}"
!define PLUGINSPATH "{{additional_plugins_path}}"
!define ALLOWDOWNGRADES "{{allow_downgrades}}"
!define DISPLAYLANGUAGESELECTOR "{{display_language_selector}}"
!define INSTALLWEBVIEW2MODE "{{install_webview2_mode}}"
!define WEBVIEW2INSTALLERARGS "{{webview2_installer_args}}"
!define WEBVIEW2BOOTSTRAPPERPATH "{{webview2_bootstrapper_path}}"
!define WEBVIEW2INSTALLERPATH "{{webview2_installer_path}}"
!define MINIMUMWEBVIEW2VERSION "{{minimum_webview2_version}}"
!define UNINSTKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCTNAME}"
!define MANUPRODUCTKEY "Software\${MANUFACTURER}\${PRODUCTNAME}"
!define UNINSTALLERSIGNCOMMAND "{{uninstaller_sign_cmd}}"
!define ESTIMATEDSIZE "{{estimated_size}}"

Name "${PRODUCTNAME}"
BrandingText "${COPYRIGHT}"
OutFile "${OUTFILE}"

VIProductVersion "${VERSIONWITHBUILD}"
VIAddVersionKey "ProductName" "${PRODUCTNAME}"
VIAddVersionKey "FileDescription" "${PRODUCTNAME}"
VIAddVersionKey "LegalCopyright" "${COPYRIGHT}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"

; Plugins path, currently exists for linux only
!if "${PLUGINSPATH}" != ""
    !addplugindir "${PLUGINSPATH}"
!endif

!if "${UNINSTALLERSIGNCOMMAND}" != ""
  !uninstfinalize '${UNINSTALLERSIGNCOMMAND}'
!endif

; Handle install mode, `perUser`, `perMachine` or `both`
!if "${INSTALLMODE}" == "perMachine"
  RequestExecutionLevel highest
!endif

!if "${INSTALLMODE}" == "currentUser"
  RequestExecutionLevel user
!endif

!if "${INSTALLMODE}" == "both"
  !define MULTIUSER_MUI
  !define MULTIUSER_INSTALLMODE_INSTDIR "${PRODUCTNAME}"
  !define MULTIUSER_INSTALLMODE_COMMANDLINE
  !if "${ARCH}" == "x64"
    !define MULTIUSER_USE_PROGRAMFILES64
  !else if "${ARCH}" == "arm64"
    !define MULTIUSER_USE_PROGRAMFILES64
  !endif
  !define MULTIUSER_INSTALLMODE_DEFAULT_REGISTRY_KEY "${UNINSTKEY}"
  !define MULTIUSER_INSTALLMODE_DEFAULT_REGISTRY_VALUENAME "CurrentUser"
  !define MULTIUSER_INSTALLMODEPAGE_SHOWUSERNAME
  !define MULTIUSER_INSTALLMODE_FUNCTION RestorePreviousInstallLocation
  !define MULTIUSER_EXECUTIONLEVEL Highest
  !include MultiUser.nsh
!endif

; installer icon
!if "${INSTALLERICON}" != ""
  !define MUI_ICON "${INSTALLERICON}"
!endif

; installer sidebar image
!if "${SIDEBARIMAGE}" != ""
  !define MUI_WELCOMEFINISHPAGE_BITMAP "${SIDEBARIMAGE}"
!endif

; installer header image
!if "${HEADERIMAGE}" != ""
  !define MUI_HEADERIMAGE
  !define MUI_HEADERIMAGE_BITMAP  "${HEADERIMAGE}"
!endif

; Define registry key to store installer language
!define MUI_LANGDLL_REGISTRY_ROOT "HKCU"
!define MUI_LANGDLL_REGISTRY_KEY "${MANUPRODUCTKEY}"
!define MUI_LANGDLL_REGISTRY_VALUENAME "Installer Language"

; Installer pages, must be ordered as they appear
; 1. Welcome Page
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
!insertmacro MUI_PAGE_WELCOME

; 2. License Page (if defined)
!if "${LICENSE}" != ""
  !define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
  !insertmacro MUI_PAGE_LICENSE "${LICENSE}"
!endif

; 3. Install mode (if it is set to `both`)
!if "${INSTALLMODE}" == "both"
  !define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
  !insertmacro MULTIUSER_PAGE_INSTALLMODE
!endif


; 4. Custom page to ask user if he wants to reinstall/uninstall
;    only if a previous installtion was detected
Var ReinstallPageCheck
Page custom PageReinstall PageLeaveReinstall
Function PageReinstall
  ; Uninstall previous WiX installation if exists.
  ;
  ; A WiX installer stores the isntallation info in registry
  ; using a UUID and so we have to loop through all keys under
  ; `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall`
  ; and check if `DisplayName` and `Publisher` keys match ${PRODUCTNAME} and ${MANUFACTURER}
  ;
  ; This has a potentional issue that there maybe another installation that matches
  ; our ${PRODUCTNAME} and ${MANUFACTURER} but wasn't installed by our WiX installer,
  ; however, this should be fine since the user will have to confirm the uninstallation
  ; and they can chose to abort it if doesn't make sense.
  StrCpy $0 0
  wix_loop:
    EnumRegKey $1 HKLM "SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall" $0
    StrCmp $1 "" wix_done ; Exit loop if there is no more keys to loop on
    IntOp $0 $0 + 1
    ReadRegStr $R0 HKLM "SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\$1" "DisplayName"
    ReadRegStr $R1 HKLM "SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\$1" "Publisher"
    StrCmp "$R0$R1" "${PRODUCTNAME}${MANUFACTURER}" 0 wix_loop
    ReadRegStr $R0 HKLM "SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\$1" "UninstallString"
    ${StrCase} $R1 $R0 "L"
    ${StrLoc} $R0 $R1 "msiexec" ">"
    StrCmp $R0 0 0 wix_done
    StrCpy $R7 "wix"
    StrCpy $R6 "SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\$1"
    Goto compare_version
  wix_done:

  ; Check if there is an existing installation, if not, abort the reinstall page
  ReadRegStr $R0 SHCTX "${UNINSTKEY}" ""
  ReadRegStr $R1 SHCTX "${UNINSTKEY}" "UninstallString"
  ${IfThen} "$R0$R1" == "" ${|} Abort ${|}

  ; Compare this installar version with the existing installation
  ; and modify the messages presented to the user accordingly
  compare_version:
  StrCpy $R4 "$(older)"
  ${If} $R7 == "wix"
    ReadRegStr $R0 HKLM "$R6" "DisplayVersion"
  ${Else}
    ReadRegStr $R0 SHCTX "${UNINSTKEY}" "DisplayVersion"
  ${EndIf}
  ${IfThen} $R0 == "" ${|} StrCpy $R4 "$(unknown)" ${|}

  nsis_tauri_utils::SemverCompare "${VERSION}" $R0
  Pop $R0
  ; Reinstalling the same version
  ${If} $R0 == 0
    StrCpy $R1 "$(alreadyInstalledLong)"
    StrCpy $R2 "$(addOrReinstall)"
    StrCpy $R3 "$(uninstallApp)"
    !insertmacro MUI_HEADER_TEXT "$(alreadyInstalled)" "$(chooseMaintenanceOption)"
    StrCpy $R5 "2"
  ; Upgrading
  ${ElseIf} $R0 == 1
    StrCpy $R1 "$(olderOrUnknownVersionInstalled)"
    StrCpy $R2 "$(uninstallBeforeInstalling)"
    StrCpy $R3 "$(dontUninstall)"
    !insertmacro MUI_HEADER_TEXT "$(alreadyInstalled)" "$(choowHowToInstall)"
    StrCpy $R5 "1"
  ; Downgrading
  ${ElseIf} $R0 == -1
    StrCpy $R1 "$(newerVersionInstalled)"
    StrCpy $R2 "$(uninstallBeforeInstalling)"
    !if "${ALLOWDOWNGRADES}" == "true"
      StrCpy $R3 "$(dontUninstall)"
    !else
      StrCpy $R3 "$(dontUninstallDowngrade)"
    !endif
    !insertmacro MUI_HEADER_TEXT "$(alreadyInstalled)" "$(choowHowToInstall)"
    StrCpy $R5 "1"
  ${Else}
    Abort
  ${EndIf}

  Call SkipIfPassive

  nsDialogs::Create 1018
  Pop $R4
  ${IfThen} $(^RTL) == 1 ${|} nsDialogs::SetRTL $(^RTL) ${|}

  ${NSD_CreateLabel} 0 0 100% 24u $R1
  Pop $R1

  ${NSD_CreateRadioButton} 30u 50u -30u 8u $R2
  Pop $R2
  ${NSD_OnClick} $R2 PageReinstallUpdateSelection

  ${NSD_CreateRadioButton} 30u 70u -30u 8u $R3
  Pop $R3
  ; disable this radio button if downgrading and downgrades are disabled
  !if "${ALLOWDOWNGRADES}" == "false"
    ${IfThen} $R0 == -1 ${|} EnableWindow $R3 0 ${|}
  !endif
  ${NSD_OnClick} $R3 PageReinstallUpdateSelection

  ; A version upgrade must be in-place. Running the old uninstaller here would
  ; stop and remove the service before the directory/start-menu pages, allowing
  ; an unattended remote machine to remain offline while the wizard waits for
  ; input. The Install section performs the required process stop immediately
  ; before copying files and restores the service before slow optional work.
  ${If} $R5 == "1"
    EnableWindow $R2 0
    SendMessage $R3 ${BM_SETCHECK} ${BST_CHECKED} 0
    StrCpy $ReinstallPageCheck 2
  ${EndIf}

  ; Check the first radio button if this the first time
  ; we enter this page or if the second button wasn't
  ; selected the last time we were on this page
  ${If} $R5 == "1"
    SendMessage $R3 ${BM_SETCHECK} ${BST_CHECKED} 0
  ${ElseIf} $ReinstallPageCheck != 2
    SendMessage $R2 ${BM_SETCHECK} ${BST_CHECKED} 0
  ${Else}
    SendMessage $R3 ${BM_SETCHECK} ${BST_CHECKED} 0
  ${EndIf}

  ${NSD_SetFocus} $R2
  nsDialogs::Show
FunctionEnd
Function PageReinstallUpdateSelection
  ${NSD_GetState} $R2 $R1
  ${If} $R1 == ${BST_CHECKED}
    StrCpy $ReinstallPageCheck 1
  ${Else}
    StrCpy $ReinstallPageCheck 2
  ${EndIf}
FunctionEnd
Function PageLeaveReinstall
  ${NSD_GetState} $R2 $R1

  ; $R5 holds whether we are reinstalling the same version or not
  ; $R5 == "1" -> different versions
  ; $R5 == "2" -> same version
  ;
  ; $R1 holds the radio buttons state. its meaning is dependant on the context
  StrCmp $R5 "1" 0 +2 ; Existing install is not the same version?
    StrCmp $R1 "1" reinst_uninstall reinst_done ; $R1 == "1", then user chose to uninstall existing version, otherwise skip uninstalling
  StrCmp $R1 "1" reinst_done ; Same version? skip uninstalling

  reinst_uninstall:
    HideWindow
    ClearErrors

    ${If} $R7 == "wix"
      ReadRegStr $R1 HKLM "$R6" "UninstallString"
      ExecWait '$R1' $0
    ${Else}
      ReadRegStr $4 SHCTX "${MANUPRODUCTKEY}" ""
      ReadRegStr $R1 SHCTX "${UNINSTKEY}" "UninstallString"
      ExecWait '$R1 /P _?=$4' $0
    ${EndIf}

    BringToFront

    ${IfThen} ${Errors} ${|} StrCpy $0 2 ${|} ; ExecWait failed, set fake exit code

    ${If} $0 <> 0
    ${OrIf} ${FileExists} "$INSTDIR\${MAINBINARYNAME}.exe"
      ${If} $0 = 1 ; User aborted uninstaller?
        StrCmp $R5 "2" 0 +2 ; Is the existing install the same version?
          Quit ; ...yes, already installed, we are done
        Abort
      ${EndIf}
      MessageBox MB_ICONEXCLAMATION "$(unableToUninstall)"
      Abort
    ${Else}
      StrCpy $0 $R1 1
      ${IfThen} $0 == '"' ${|} StrCpy $R1 $R1 -1 1 ${|} ; Strip quotes from UninstallString
      Delete $R1
      RMDir $INSTDIR
    ${EndIf}
  reinst_done:
FunctionEnd

; 5. Choose install directoy page
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
!insertmacro MUI_PAGE_DIRECTORY

; 6. Start menu shortcut page
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
Var AppStartMenuFolder
!insertmacro MUI_PAGE_STARTMENU Application $AppStartMenuFolder

; 7. Installation page
!insertmacro MUI_PAGE_INSTFILES

; 8. Finish page
;
; Don't auto jump to finish page after installation page,
; because the installation page has useful info that can be used debug any issues with the installer.
!define MUI_FINISHPAGE_NOAUTOCLOSE
; Use show readme button in the finish page as a button create a desktop shortcut
!define MUI_FINISHPAGE_SHOWREADME
!define MUI_FINISHPAGE_SHOWREADME_TEXT "$(createDesktop)"
!define MUI_FINISHPAGE_SHOWREADME_FUNCTION CreateDesktopShortcut
; Show run app after installation.
!define MUI_FINISHPAGE_RUN
!define MUI_FINISHPAGE_RUN_FUNCTION RunMainBinary
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
!insertmacro MUI_PAGE_FINISH

Function RunMainBinary
  nsis_tauri_utils::RunAsUser "$INSTDIR\${MAINBINARYNAME}.exe" ""
FunctionEnd

; Uninstaller Pages
; 1. Confirm uninstall page
Var DeleteAppDataCheckbox
Var DeleteAppDataCheckboxState
!define /ifndef WS_EX_LAYOUTRTL         0x00400000
!define MUI_PAGE_CUSTOMFUNCTION_SHOW un.ConfirmShow
Function un.ConfirmShow
    FindWindow $1 "#32770" "" $HWNDPARENT ; Find inner dialog
    ${If} $(^RTL) == 1
      System::Call 'USER32::CreateWindowEx(i${__NSD_CheckBox_EXSTYLE}|${WS_EX_LAYOUTRTL},t"${__NSD_CheckBox_CLASS}",t "$(deleteAppData)",i${__NSD_CheckBox_STYLE},i 50,i 100,i 400, i 25,i$1,i0,i0,i0)i.s'
    ${Else}
      System::Call 'USER32::CreateWindowEx(i${__NSD_CheckBox_EXSTYLE},t"${__NSD_CheckBox_CLASS}",t "$(deleteAppData)",i${__NSD_CheckBox_STYLE},i 0,i 100,i 400, i 25,i$1,i0,i0,i0)i.s'
    ${EndIf}
    Pop $DeleteAppDataCheckbox
    SendMessage $HWNDPARENT ${WM_GETFONT} 0 0 $1
    SendMessage $DeleteAppDataCheckbox ${WM_SETFONT} $1 1
FunctionEnd
!define MUI_PAGE_CUSTOMFUNCTION_LEAVE un.ConfirmLeave
Function un.ConfirmLeave
    SendMessage $DeleteAppDataCheckbox ${BM_GETCHECK} 0 0 $DeleteAppDataCheckboxState
FunctionEnd
!insertmacro MUI_UNPAGE_CONFIRM

; 2. Uninstalling Page
!insertmacro MUI_UNPAGE_INSTFILES

;Languages
{{#each languages}}
!insertmacro MUI_LANGUAGE "{{this}}"
{{/each}}
!insertmacro MUI_RESERVEFILE_LANGDLL
{{#each language_files}}
  !include "{{this}}"
{{/each}}

!macro SetContext
  !if "${INSTALLMODE}" == "currentUser"
    SetShellVarContext current
  !else if "${INSTALLMODE}" == "perMachine"
    SetShellVarContext all
  !endif

  ${If} ${RunningX64}
    !if "${ARCH}" == "x64"
      SetRegView 64
    !else if "${ARCH}" == "arm64"
      SetRegView 64
    !else
      SetRegView 32
    !endif
  ${EndIf}
!macroend

Var PassiveMode
Function .onInit
  ${GetOptions} $CMDLINE "/P" $PassiveMode
  IfErrors +2 0
    StrCpy $PassiveMode 1

  !if "${DISPLAYLANGUAGESELECTOR}" == "true"
    !insertmacro MUI_LANGDLL_DISPLAY
  !endif

  !insertmacro SetContext

  ${If} $INSTDIR == ""
    ; Set default install location
    !if "${INSTALLMODE}" == "perMachine"
      ${If} ${RunningX64}
        !if "${ARCH}" == "x64"
          StrCpy $INSTDIR "$PROGRAMFILES64\${PRODUCTNAME}"
        !else if "${ARCH}" == "arm64"
          StrCpy $INSTDIR "$PROGRAMFILES64\${PRODUCTNAME}"
        !else
          StrCpy $INSTDIR "$PROGRAMFILES\${PRODUCTNAME}"
        !endif
      ${Else}
        StrCpy $INSTDIR "$PROGRAMFILES\${PRODUCTNAME}"
      ${EndIf}
    !else if "${INSTALLMODE}" == "currentUser"
      StrCpy $INSTDIR "$LOCALAPPDATA\${PRODUCTNAME}"
    !endif

    Call RestorePreviousInstallLocation
  ${EndIf}


  !if "${INSTALLMODE}" == "both"
    !insertmacro MULTIUSER_INIT
  !endif
FunctionEnd


Section EarlyChecks
  ; Abort silent installer if downgrades is disabled
  !if "${ALLOWDOWNGRADES}" == "false"
  IfSilent 0 silent_downgrades_done
    ; If downgrading
    ${If} $R0 == -1
      System::Call 'kernel32::AttachConsole(i -1)i.r0'
      ${If} $0 != 0
        System::Call 'kernel32::GetStdHandle(i -11)i.r0'
        System::call 'kernel32::SetConsoleTextAttribute(i r0, i 0x0004)' ; set red color
        FileWrite $0 "$(silentDowngrades)"
      ${EndIf}
      Abort
    ${EndIf}
  silent_downgrades_done:
  !endif

SectionEnd

Section WebView2
  ; Check if Webview2 is already installed and skip this section
  ${If} ${RunningX64}
    ReadRegStr $4 HKLM "SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" "pv"
  ${Else}
    ReadRegStr $4 HKLM "SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" "pv"
  ${EndIf}
  ReadRegStr $5 HKCU "SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" "pv"

  StrCpy $7 $4
  StrCmp $7 "" 0 +2
  StrCpy $7 $5

  ; The Win7 package pins WebView2 109. Treat an older installed runtime as
  ; missing. Modern packages leave MINIMUMWEBVIEW2VERSION empty and retain the
  ; normal Tauri v1 "install only when absent" behavior.
  !if "${INSTALLWEBVIEW2MODE}" == "offlineInstaller"
    !if "${MINIMUMWEBVIEW2VERSION}" != ""
      StrCmp $7 "" install_webview2_check_done
      ${VersionCompare} $7 "${MINIMUMWEBVIEW2VERSION}" $0
      StrCmp $0 "2" install_webview2_check_done webview2_done
      install_webview2_check_done:
    !else
      StrCmp $7 "" 0 webview2_done
    !endif
  !else
    StrCmp $7 "" 0 webview2_done
  !endif

  ; Webview2 install modes
  !if "${INSTALLWEBVIEW2MODE}" == "downloadBootstrapper"
    Delete "$TEMP\MicrosoftEdgeWebview2Setup.exe"
    DetailPrint "$(webview2Downloading)"
    NSISdl::download "https://go.microsoft.com/fwlink/p/?LinkId=2124703" "$TEMP\MicrosoftEdgeWebview2Setup.exe"
    Pop $0
    ${If} $0 == "success"
      DetailPrint "$(webview2DownloadSuccess)"
    ${Else}
      DetailPrint "$(webview2DownloadError)"
      Abort "$(webview2AbortError)"
    ${EndIf}
    StrCpy $6 "$TEMP\MicrosoftEdgeWebview2Setup.exe"
    Goto install_webview2
  !endif

  !if "${INSTALLWEBVIEW2MODE}" == "embedBootstrapper"
    Delete "$TEMP\MicrosoftEdgeWebview2Setup.exe"
    File "/oname=$TEMP\MicrosoftEdgeWebview2Setup.exe" "${WEBVIEW2BOOTSTRAPPERPATH}"
    DetailPrint "$(installingWebview2)"
    StrCpy $6 "$TEMP\MicrosoftEdgeWebview2Setup.exe"
    Goto install_webview2
  !endif

  !if "${INSTALLWEBVIEW2MODE}" == "offlineInstaller"
    Delete "$TEMP\MicrosoftEdgeWebView2RuntimeInstaller.exe"
    File "/oname=$TEMP\MicrosoftEdgeWebView2RuntimeInstaller.exe" "${WEBVIEW2INSTALLERPATH}"
    DetailPrint "$(installingWebview2)"
    StrCpy $6 "$TEMP\MicrosoftEdgeWebView2RuntimeInstaller.exe"
    Goto install_webview2
  !endif

  Goto webview2_done

  install_webview2:
    DetailPrint "$(installingWebview2)"
    ; $6 holds the path to the webview2 installer
    !if "${INSTALLWEBVIEW2MODE}" == "offlineInstaller"
      ; WebView2 共享载荷不认 /silent、/install（"unsupported switch for
      ; WebView"，退出码 53），也不需要它们——静默语义由 build-windows-win7.ps1
      ; 的参数集表达，此处不得追加 /install（其他模式的包装器仍需要它）。
      ExecWait "$6 ${WEBVIEW2INSTALLERARGS}" $1
    !else
      ExecWait "$6 ${WEBVIEW2INSTALLERARGS} /install" $1
    !endif
    ${If} $1 != 0
      DetailPrint "$(webview2InstallError)"
      Abort "$(webview2AbortError)"
    ${EndIf}
    !if "${INSTALLWEBVIEW2MODE}" == "offlineInstaller"
      ${If} ${RunningX64}
        ReadRegStr $7 HKLM "SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" "pv"
      ${Else}
        ReadRegStr $7 HKLM "SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" "pv"
      ${EndIf}
      StrCmp $7 "" 0 +2
      Abort "WebView2 Runtime was not installed"
      !if "${MINIMUMWEBVIEW2VERSION}" != ""
        ${VersionCompare} $7 "${MINIMUMWEBVIEW2VERSION}" $0
        ${If} $0 == "2"
          Abort "Installed WebView2 Runtime is older than ${MINIMUMWEBVIEW2VERSION}"
        ${EndIf}
      !endif
    !endif
    DetailPrint "$(webview2InstallSuccess)"
  webview2_done:
SectionEnd

!macro CheckIfAppIsRunning
  !if "${INSTALLMODE}" == "currentUser"
    nsis_tauri_utils::FindProcessCurrentUser "${MAINBINARYNAME}.exe"
  !else
    nsis_tauri_utils::FindProcess "${MAINBINARYNAME}.exe"
  !endif
  Pop $R0
  ${If} $R0 = 0
      IfSilent kill 0
      ${IfThen} $PassiveMode != 1 ${|} MessageBox MB_OKCANCEL "$(appRunningOkKill)" IDOK kill IDCANCEL cancel ${|}
      kill:
        !if "${INSTALLMODE}" == "currentUser"
          nsis_tauri_utils::KillProcessCurrentUser "${MAINBINARYNAME}.exe"
        !else
          nsis_tauri_utils::KillProcess "${MAINBINARYNAME}.exe"
        !endif
        Pop $R0
        Sleep 500
        ${If} $R0 = 0
          Goto app_check_done
        ${Else}
          IfSilent silent ui
          silent:
            System::Call 'kernel32::AttachConsole(i -1)i.r0'
            ${If} $0 != 0
              System::Call 'kernel32::GetStdHandle(i -11)i.r0'
              System::call 'kernel32::SetConsoleTextAttribute(i r0, i 0x0004)' ; set red color
              FileWrite $0 "$(appRunning)$\n"
            ${EndIf}
            Abort
          ui:
            Abort "$(failedToKillApp)"
        ${EndIf}
      cancel:
        Abort "$(appRunning)"
  ${EndIf}
  app_check_done:
!macroend

Section Install
  SetOutPath $INSTDIR

  !insertmacro CheckIfAppIsRunning
  !insertmacro NSIS_HOOK_PREINSTALL

  ; Copy main executable
  File "${MAINBINARYSRCPATH}"

  ; Copy resources
  {{#each resources_dirs}}
    CreateDirectory "$INSTDIR\\{{this}}"
  {{/each}}
  {{#each resources}}
    File /a "/oname={{this.[1]}}" "{{unescape-dollar-sign @key}}"
  {{/each}}

  ; Copy external binaries
  {{#each binaries}}
    File /a "/oname={{this}}" "{{unescape-dollar-sign @key}}"
  {{/each}}

  ; Create uninstaller
  WriteUninstaller "$INSTDIR\uninstall.exe"

  ; Save $INSTDIR in registry for future installations
  WriteRegStr SHCTX "${MANUPRODUCTKEY}" "" $INSTDIR

  !if "${INSTALLMODE}" == "both"
    ; Save install mode to be selected by default for the next installation such as updating
    ; or when uninstalling
    WriteRegStr SHCTX "${UNINSTKEY}" $MultiUser.InstallMode 1
  !endif

  ; Save current MAINBINARYNAME for future updates from v2 updater
  WriteRegStr SHCTX "${UNINSTKEY}" "MainBinaryName" "${MAINBINARYNAME}.exe"

  ; Registry information for add/remove programs
  WriteRegStr SHCTX "${UNINSTKEY}" "DisplayName" "${PRODUCTNAME}"
  WriteRegStr SHCTX "${UNINSTKEY}" "DisplayIcon" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\""
  WriteRegStr SHCTX "${UNINSTKEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr SHCTX "${UNINSTKEY}" "Publisher" "${MANUFACTURER}"
  WriteRegStr SHCTX "${UNINSTKEY}" "InstallLocation" "$\"$INSTDIR$\""
  WriteRegStr SHCTX "${UNINSTKEY}" "UninstallString" "$\"$INSTDIR\uninstall.exe$\""
  WriteRegDWORD SHCTX "${UNINSTKEY}" "NoModify" "1"
  WriteRegDWORD SHCTX "${UNINSTKEY}" "NoRepair" "1"
  WriteRegDWORD SHCTX "${UNINSTKEY}" "EstimatedSize" "${ESTIMATEDSIZE}"

  ; Create start menu shortcut (GUI)
  !insertmacro MUI_STARTMENU_WRITE_BEGIN Application
    Call CreateStartMenuShortcut
  !insertmacro MUI_STARTMENU_WRITE_END

  ; Create shortcuts for silent and passive installers, which
  ; can be disabled by passing `/NS` flag
  ; GUI installer has buttons for users to control creating them
  IfSilent check_ns_flag 0
  ${IfThen} $PassiveMode == 1 ${|} Goto check_ns_flag ${|}
  Goto shortcuts_done
  check_ns_flag:
    ${GetOptions} $CMDLINE "/NS" $R0
    IfErrors 0 shortcuts_done
      Call CreateDesktopShortcut
      Call CreateStartMenuShortcut
  shortcuts_done:

  ; Auto close this page for passive mode
  ${IfThen} $PassiveMode == 1 ${|} SetAutoClose true ${|}
  !insertmacro NSIS_HOOK_POSTINSTALL
SectionEnd

Function .onInstSuccess
  ; Check for `/R` flag only in silent and passive installers because
  ; GUI installer has a toggle for the user to (re)start the app
  ; A silent in-place update can report success while SCM still has the
  ; service stopped.  Issue one final native SCM start after all NSIS sections
  ; have completed; ERROR_SERVICE_ALREADY_RUNNING is harmless and ignored.
  IfSilent ensure_service_running 0
  ${IfThen} $PassiveMode == 1 ${|} Goto check_r_flag ${|}
  Goto run_done
  ensure_service_running:
    ExecWait 'sc.exe start p2premote-service' $R8
    DetailPrint "p2premote-service success-hook start exit code: $R8"
  check_r_flag:
    ${GetOptions} $CMDLINE "/R" $R0
    IfErrors run_done 0
      ${GetOptions} $CMDLINE "/ARGS" $R0
      nsis_tauri_utils::RunAsUser "$INSTDIR\${MAINBINARYNAME}.exe" "$R0"
  run_done:
FunctionEnd

Function un.onInit
  !insertmacro SetContext

  !if "${INSTALLMODE}" == "both"
    !insertmacro MULTIUSER_UNINIT
  !endif

  !insertmacro MUI_UNGETLANGUAGE
FunctionEnd

!macro DeleteAppUserModelId
  !insertmacro ComHlpr_CreateInProcInstance ${CLSID_DestinationList} ${IID_ICustomDestinationList} r1 ""
  ${If} $1 P<> 0
    ${ICustomDestinationList::DeleteList} $1 '("${BUNDLEID}")'
    ${IUnknown::Release} $1 ""
  ${EndIf}
  !insertmacro ComHlpr_CreateInProcInstance ${CLSID_ApplicationDestinations} ${IID_IApplicationDestinations} r1 ""
  ${If} $1 P<> 0
    ${IApplicationDestinations::SetAppID} $1 '("${BUNDLEID}")i.r0'
    ${If} $0 >= 0
      ${IApplicationDestinations::RemoveAllDestinations} $1 ''
    ${EndIf}
    ${IUnknown::Release} $1 ""
  ${EndIf}
!macroend

; From https://stackoverflow.com/a/42816728/16993372
!macro UnpinShortcut shortcut
  !insertmacro ComHlpr_CreateInProcInstance ${CLSID_StartMenuPin} ${IID_IStartMenuPinnedList} r0 ""
  ${If} $0 P<> 0
      System::Call 'SHELL32::SHCreateItemFromParsingName(ws, p0, g "${IID_IShellItem}", *p0r1)' "${shortcut}"
      ${If} $1 P<> 0
          ${IStartMenuPinnedList::RemoveFromList} $0 '(r1)'
          ${IUnknown::Release} $1 ""
      ${EndIf}
      ${IUnknown::Release} $0 ""
  ${EndIf}
!macroend

Section Uninstall
  !insertmacro CheckIfAppIsRunning
  !insertmacro NSIS_HOOK_PREUNINSTALL

  ; Delete the app directory and its content from disk
  ; Copy main executable
  Delete "$INSTDIR\${MAINBINARYNAME}.exe"

  ; Delete resources
  {{#each resources}}
    Delete "$INSTDIR\\{{this.[1]}}"
  {{/each}}

  ; Delete external binaries
  {{#each binaries}}
    Delete "$INSTDIR\\{{this}}"
  {{/each}}

  ; Delete uninstaller
  Delete "$INSTDIR\uninstall.exe"

  {{#each resources_ancestors}}
  RMDir /REBOOTOK "$INSTDIR\\{{this}}"
  {{/each}}
  RMDir "$INSTDIR"

  !insertmacro DeleteAppUserModelId
  !insertmacro UnpinShortcut "$SMPROGRAMS\$AppStartMenuFolder\${MAINBINARYNAME}.lnk"
  !insertmacro UnpinShortcut "$DESKTOP\${MAINBINARYNAME}.lnk"

  ; Remove start menu shortcut
  !insertmacro MUI_STARTMENU_GETFOLDER Application $AppStartMenuFolder
  Delete "$SMPROGRAMS\$AppStartMenuFolder\${MAINBINARYNAME}.lnk"
  RMDir "$SMPROGRAMS\$AppStartMenuFolder"

  ; Remove desktop shortcuts
  Delete "$DESKTOP\${MAINBINARYNAME}.lnk"

  ; Remove registry information for add/remove programs
  !if "${INSTALLMODE}" == "both"
    DeleteRegKey SHCTX "${UNINSTKEY}"
  !else if "${INSTALLMODE}" == "perMachine"
    DeleteRegKey HKLM "${UNINSTKEY}"
  !else
    DeleteRegKey HKCU "${UNINSTKEY}"
  !endif

  DeleteRegValue HKCU "${MANUPRODUCTKEY}" "Installer Language"

  ; Delete app data
  ${If} $DeleteAppDataCheckboxState == 1
    SetShellVarContext current
    RmDir /r "$APPDATA\${BUNDLEID}"
    RmDir /r "$LOCALAPPDATA\${BUNDLEID}"
  ${EndIf}

  ${GetOptions} $CMDLINE "/P" $R0
  IfErrors +2 0
    SetAutoClose true
  !insertmacro NSIS_HOOK_POSTUNINSTALL
SectionEnd

Function RestorePreviousInstallLocation
  ReadRegStr $4 SHCTX "${MANUPRODUCTKEY}" ""
  StrCmp $4 "" +2 0
    StrCpy $INSTDIR $4
FunctionEnd

Function SkipIfPassive
  ${IfThen} $PassiveMode == 1  ${|} Abort ${|}
FunctionEnd

!macro SetLnkAppUserModelId shortcut
  !insertmacro ComHlpr_CreateInProcInstance ${CLSID_ShellLink} ${IID_IShellLink} r0 ""
  ${If} $0 P<> 0
    ${IUnknown::QueryInterface} $0 '("${IID_IPersistFile}",.r1)'
    ${If} $1 P<> 0
      ${IPersistFile::Load} $1 '("${shortcut}", ${STGM_READWRITE})'
      ${IUnknown::QueryInterface} $0 '("${IID_IPropertyStore}",.r2)'
      ${If} $2 P<> 0
        System::Call 'Oleaut32::SysAllocString(w "${BUNDLEID}") i.r3'
        System::Call '*${SYSSTRUCT_PROPERTYKEY}(${PKEY_AppUserModel_ID})p.r4'
        System::Call '*${SYSSTRUCT_PROPVARIANT}(${VT_BSTR},,&i4 $3)p.r5'
        ${IPropertyStore::SetValue} $2 '($4,$5)'

        System::Call 'Oleaut32::SysFreeString($3)'
        System::Free $4
        System::Free $5
        ${IPropertyStore::Commit} $2 ""
        ${IUnknown::Release} $2 ""
        ${IPersistFile::Save} $1 '("${shortcut}",1)'
      ${EndIf}
      ${IUnknown::Release} $1 ""
    ${EndIf}
    ${IUnknown::Release} $0 ""
  ${EndIf}
!macroend

Function CreateDesktopShortcut
  CreateShortcut "$DESKTOP\${MAINBINARYNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
  !insertmacro SetLnkAppUserModelId "$DESKTOP\${MAINBINARYNAME}.lnk"
FunctionEnd

Function CreateStartMenuShortcut
  CreateDirectory "$SMPROGRAMS\$AppStartMenuFolder"
  CreateShortcut "$SMPROGRAMS\$AppStartMenuFolder\${MAINBINARYNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
  !insertmacro SetLnkAppUserModelId "$SMPROGRAMS\$AppStartMenuFolder\${MAINBINARYNAME}.lnk"
FunctionEnd
