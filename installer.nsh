; Hooks for Tauri's NSIS installer (see tauri.conf.json > bundle > windows > nsis).

; The uninstaller only removes files it installed itself. flagra also writes
; files next to its exe (its logs), and older versions' install.ps1 put a
; downloaded ffmpeg in ffmpeg\, so clear those too.
; Updates run the old uninstaller too, so skip all this then. flagra.ini, the
; user's settings, always stays; clips live in Videos\flagra and are never touched.
!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    ; flagra adds itself here for "start with Windows" (not the installer), so remove it too.
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "flagra"
    RMDir /r "$INSTDIR\ffmpeg"
    Delete "$INSTDIR\flagra.log"
    Delete "$INSTDIR\ffmpeg.log"
    RMDir "$INSTDIR"
  ${EndIf}
!macroend
