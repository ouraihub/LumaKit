@echo off
chcp 65001 >nul
setlocal
for %%R in ("%~dp0..") do set "ROOT=%%~fR\"
set "EXE=%ROOT%dist\lumakit\lumakit-cli.exe"
if not exist "%EXE%" (
  echo EXE not found. Run scripts\build-app.bat first. 1>&2
  exit /b 1
)
"%EXE%" %*
exit /b %errorlevel%
