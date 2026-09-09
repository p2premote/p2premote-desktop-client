; NSIS installer hooks for p2premote-service
; perMachine install mode runs as admin, no UAC needed

; Overwrite-install convenience: a bare double-click of the setup exe on a
; machine that already has p2pRemote first warns that existing tunnels will be
; disconnected, then reruns itself with /S /UPDATE /R after confirmation
; (silent update, no WebView2 download, restart app as the logged-in user after
; install). Fresh installs and launches that already carry /S or /P
; (service- or app-triggered updates) keep their behavior.
; This file is included before the template defines PRODUCTNAME and its Vars,
; so the function below must not reference them: the registry key and binary
; name are spelled out and must match tauri.conf.json
; (productName=p2pRemote, mainBinaryName=p2premote, manufacturer=p2premote).
!define MUI_CUSTOMFUNCTION_GUIINIT P2PRemoteAutoSilentUpdateInit

Function P2PRemoteAutoSilentUpdateInit
  ; .onGUIInit never runs in silent mode; guard anyway for future template changes.
  IfSilent p2pr_asu_done

  ; An explicit /P (passive, progress-only) already avoids the wizard.
  ${GetOptions} $CMDLINE "/P" $R9
  IfErrors p2pr_asu_check p2pr_asu_done

  p2pr_asu_check:
  ; The template records the install dir as the default value of
  ; HKLM\Software\<manufacturer>\<productName> on every install.
  ReadRegStr $0 HKLM "Software\p2premote\p2pRemote" ""
  StrCmp $0 "" p2pr_asu_done
  IfFileExists "$0\p2premote.exe" 0 p2pr_asu_done

  ; Updating replaces and restarts the tunnel-owning service, so require an
  ; explicit acknowledgement before switching to the unattended update flow.
  MessageBox MB_YESNO|MB_ICONEXCLAMATION \
    "The update will disconnect existing tunnel connections. After the update is complete, please reconnect the tunnels." \
    IDYES p2pr_asu_confirmed IDNO p2pr_asu_done

  p2pr_asu_confirmed:

  ; Relaunch as silent updater; the child inherits this process's elevated
  ; token, so no second UAC prompt appears.
  StrCpy $R8 $EXEPATH
  StrCpy $R9 $R8 1
  StrCmp $R9 '"' +2 0
  StrCpy $R8 '"$EXEPATH"'
  Exec '$R8 /S /UPDATE /R'
  Quit

  p2pr_asu_done:
FunctionEnd

!macro NSIS_HOOK_PREINSTALL
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
  ; Allow authenticated WGVPN peers to reach the persistent TCP health endpoint.
  ; Restrict the rule to the RFC 6598 virtual network instead of exposing the port globally.
  nsExec::ExecToLog 'netsh advfirewall firewall delete rule name="p2pRemote WGVPN Health"'
  nsExec::ExecToLog 'netsh advfirewall firewall add rule name="p2pRemote WGVPN Health" dir=in action=allow protocol=TCP localport=48082 remoteip=100.64.0.0/10 program="$INSTDIR\resources\p2premote-service.exe" profile=any enable=yes'
  nsExec::ExecToLog 'netsh advfirewall firewall delete rule name="p2pRemote WGVPN Speed Test"'
  nsExec::ExecToLog 'netsh advfirewall firewall add rule name="p2pRemote WGVPN Speed Test" dir=in action=allow protocol=UDP localport=48082 remoteip=100.64.0.0/10 program="$INSTDIR\resources\p2premote-service.exe" profile=any enable=yes'
  nsExec::ExecToLog 'netsh advfirewall firewall delete rule name="p2pRemote Desktop Engine"'
  nsExec::ExecToLog 'netsh advfirewall firewall add rule name="p2pRemote Desktop Engine" dir=in action=allow protocol=TCP localport=39090 remoteip=100.64.0.0/10 program="$INSTDIR\resources\p2premote-desktop-engine.exe" profile=any enable=yes'

  ; Probe the persisted auto_start flag before touching the service.
  ; The uninstall phase of a reinstall deletes the HKCU Run autostart entries
  ; and the service itself, while data\config.json survives by default and only
  ; stores non-default values (the key is present only when the user enabled it).
  StrCpy $1 "unknown"
  IfFileExists "$INSTDIR\data\config.json" 0 autorun_probed
  ; Note: the trailing space is required — NSIS does not expand a $\" escape
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

  ; Respect a persisted auto_start=off: setup force-enables AutoStart, so drop
  ; the boot autostart again — a reinstall must not silently revert the choice.
  ${If} $1 == "off"
    ExecWait '"$INSTDIR\resources\p2premote-service.exe" --scm disable' $0
    DetailPrint "p2premote-service disable exit code: $0"
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::ExecToLog 'netsh advfirewall firewall delete rule name="p2pRemote WGVPN Health"'
  nsExec::ExecToLog 'netsh advfirewall firewall delete rule name="p2pRemote WGVPN Speed Test"'
  nsExec::ExecToLog 'netsh advfirewall firewall delete rule name="p2pRemote Desktop Engine"'

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
  Delete "$INSTDIR\resources\wintun.dll"
  RMDir "$INSTDIR\resources"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "p2premote-notifier"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; p2pRemote stores machine-wide runtime data under the per-machine install root,
  ; while Tauri's built-in checkbox only removes APPDATA/LOCALAPPDATA paths.
  ; Respect the same checkbox and never delete data during an in-place update.
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    DetailPrint "Removing p2pRemote application data..."
    RMDir /r /REBOOTOK "$INSTDIR\data"
    RMDir /REBOOTOK "$INSTDIR"
  ${EndIf}
!macroend
