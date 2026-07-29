; NSIS installer hooks for p2premote-service
; perMachine install mode runs as admin, no UAC needed

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

  ; Install + enable(AutoStart) + start the service
  ExecWait '"$INSTDIR\resources\p2premote-service.exe" --scm setup "$INSTDIR\resources\p2premote-service.exe"' $0
  DetailPrint "p2premote-service setup exit code: $0"
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
