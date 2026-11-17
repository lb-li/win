@echo off
setlocal
cd /d "%~dp0.."

echo [1/3] building release binary ...
cargo build --release -p fsearch-ui
if errorlevel 1 (
  echo cargo build failed
  exit /b 1
)

echo [2/3] staging dist ...
if not exist "dist" mkdir "dist"
if not exist "target\release\fsearch.exe" (
  echo missing target\release\fsearch.exe
  exit /b 1
)
copy /y "target\release\fsearch.exe" "dist\fsearch.exe" >nul

echo [3/3] done
echo OK: %CD%\dist\fsearch.exe
endlocal