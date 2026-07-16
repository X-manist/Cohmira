Unicode true
RequestExecutionLevel user
SetCompressor /SOLID lzma
SetCompressorDictSize 64

!include "MUI2.nsh"

!define PRODUCT_NAME "商媒运营助手"
!define PRODUCT_VERSION "0.1.0"
!define PRODUCT_EXE "yunying-desktop.exe"
!cd "${__FILEDIR__}"
!define DESKTOP_EXE "../../target/x86_64-pc-windows-msvc/release/yunying-desktop.exe"
!define OPS_MCP_EXE "../../../target/x86_64-pc-windows-msvc/release/yunying-ops-mcp.exe"
!define CONFIG_FILE "../../../config.json.example"
!define PLUGIN_DIR "../../../builtin-plugins"
!define FFMPEG_RUNTIME_DIR "../../runtime/ffmpeg"
!define PYTHON_RUNTIME_DIR "../../runtime/python"
!define RUNTIME_MANIFEST "../../runtime/windows-runtime-manifest.json"
!define OUTPUT_FILE "../../target/x86_64-pc-windows-msvc/release/bundle/nsis/商媒运营助手_0.1.0_x64-setup.exe"

Name "${PRODUCT_NAME} ${PRODUCT_VERSION}"
OutFile "${OUTPUT_FILE}"
InstallDir "$LOCALAPPDATA\Programs\${PRODUCT_NAME}"
InstallDirRegKey HKCU "Software\${PRODUCT_NAME}" "InstallDir"
ShowInstDetails show
ShowUninstDetails show

!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "SimpChinese"

Section "Install"
  SetOutPath "$INSTDIR"
  File "${DESKTOP_EXE}"
  File /oname=config.json "${CONFIG_FILE}"

  SetOutPath "$INSTDIR\bin"
  File "${OPS_MCP_EXE}"

  SetOutPath "$INSTDIR\builtin-plugins"
  File /r "${PLUGIN_DIR}\*.*"

  SetOutPath "$INSTDIR\ffmpeg-runtime"
  File /r "${FFMPEG_RUNTIME_DIR}\*.*"

  SetOutPath "$INSTDIR\python-runtime"
  File /r "${PYTHON_RUNTIME_DIR}\*.*"

  SetOutPath "$INSTDIR"
  File "${RUNTIME_MANIFEST}"

  WriteRegStr HKCU "Software\${PRODUCT_NAME}" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_NAME}" "DisplayName" "${PRODUCT_NAME}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_NAME}" "DisplayVersion" "${PRODUCT_VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_NAME}" "DisplayIcon" "$INSTDIR\${PRODUCT_EXE}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_NAME}" "UninstallString" "$INSTDIR\Uninstall.exe"
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_NAME}" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_NAME}" "NoRepair" 1

  CreateDirectory "$SMPROGRAMS\${PRODUCT_NAME}"
  CreateShortcut "$SMPROGRAMS\${PRODUCT_NAME}\${PRODUCT_NAME}.lnk" "$INSTDIR\${PRODUCT_EXE}"
  CreateShortcut "$SMPROGRAMS\${PRODUCT_NAME}\卸载 ${PRODUCT_NAME}.lnk" "$INSTDIR\Uninstall.exe"
  CreateShortcut "$DESKTOP\${PRODUCT_NAME}.lnk" "$INSTDIR\${PRODUCT_EXE}"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
SectionEnd

Section "Uninstall"
  Delete "$DESKTOP\${PRODUCT_NAME}.lnk"
  Delete "$SMPROGRAMS\${PRODUCT_NAME}\${PRODUCT_NAME}.lnk"
  Delete "$SMPROGRAMS\${PRODUCT_NAME}\卸载 ${PRODUCT_NAME}.lnk"
  RMDir "$SMPROGRAMS\${PRODUCT_NAME}"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_NAME}"
  DeleteRegKey HKCU "Software\${PRODUCT_NAME}"
  RMDir /r "$INSTDIR"
SectionEnd
