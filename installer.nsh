; Hooks for Tauri's NSIS installer (see tauri.conf.json > bundle > windows > nsis).

; The uninstaller only removes files it installed itself. flagra also keeps
; files next to its exe: the ffmpeg\ folder install.ps1 downloads and its logs.
; Updates run the old uninstaller too, so leave them alone then (re-downloading
; ffmpeg on every update would be a waste). flagra.ini, the user's settings,
; always stays; clips live in Videos\flagra and are never touched.
!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    RMDir /r "$INSTDIR\ffmpeg"
    Delete "$INSTDIR\flagra.log"
    Delete "$INSTDIR\ffmpeg.log"
    RMDir "$INSTDIR"
  ${EndIf}
!macroend
