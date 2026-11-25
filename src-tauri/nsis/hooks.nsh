; Extra installer hooks for CreatorFloww.
; Desktop shortcut is created for all users on per-machine installs.

!macro NSIS_HOOK_POSTINSTALL
  SetOutPath "$INSTDIR"
  CreateShortCut "$DESKTOP\CreatorFloww.lnk" "$INSTDIR\creatorfloww.exe" "" "$INSTDIR\creatorfloww.exe" 0
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  Delete "$DESKTOP\CreatorFloww.lnk"
!macroend
