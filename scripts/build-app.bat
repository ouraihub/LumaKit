@echo off
chcp 65001 >nul
setlocal

rem Build the LumaKit CLI release EXE and collect it under dist\lumakit\.
for %%R in ("%~dp0..") do set "ROOT=%%~fR\"
set "SRC=%ROOT%target\release\lumakit-cli.exe"
set "DIST=%ROOT%dist\lumakit"
set "MSVC_ROOT=%PORTABLE_MSVC_ROOT%"
if "%MSVC_ROOT%"=="" set "MSVC_ROOT=D:\Applications\msvc-portable\msvc"

echo ============================================
echo   LumaKit - release build
echo ============================================
echo.

echo [1/3] Locating MSVC toolchain...
where /q cl.exe
if not errorlevel 1 goto msvc_ok
if not exist "%MSVC_ROOT%\setup_x64.bat" (
  echo ERROR: MSVC toolchain not found: %MSVC_ROOT%\setup_x64.bat 1>&2
  exit /b 1
)
call "%MSVC_ROOT%\setup_x64.bat" >nul
where /q cl.exe
if errorlevel 1 (
  echo ERROR: cl.exe is not available after MSVC setup. 1>&2
  exit /b 1
)

:msvc_ok
echo [2/3] Building release EXE...
call cargo +1.99.0 build --release -p lumakit-cli
if errorlevel 1 (
  echo ERROR: cargo release build failed. 1>&2
  exit /b 1
)
if not exist "%SRC%" (
  echo ERROR: build succeeded but EXE is missing: %SRC% 1>&2
  exit /b 1
)

echo [3/3] Collecting artifact...
if not exist "%DIST%" mkdir "%DIST%"
copy /y "%SRC%" "%DIST%\lumakit-cli.exe" >nul
if errorlevel 1 (
  echo ERROR: failed to copy the EXE. Close any running LumaKit process and retry. 1>&2
  exit /b 1
)

echo.
echo Done.
echo EXE: %DIST%\lumakit-cli.exe
echo Data: %DIST%\data\
exit /b 0
