@echo off
:: ai for PowerShell / CMD on Windows: runs bin\ai with Git Bash (part of Git for Windows,
:: which Claude Code on Windows needs anyway).
setlocal
set "AI_BASH=%CLAUDE_CODE_GIT_BASH_PATH%"
if not defined AI_BASH for /f "delims=" %%g in ('where git 2^>nul') do if not defined AI_BASH if exist "%%~dpg..\bin\bash.exe" set "AI_BASH=%%~dpg..\bin\bash.exe"
if not defined AI_BASH if exist "%ProgramFiles%\Git\bin\bash.exe" set "AI_BASH=%ProgramFiles%\Git\bin\bash.exe"
if not defined AI_BASH (
  echo ai needs Git for Windows ^(Git Bash^): https://git-scm.com/download/win
  exit /b 1
)
"%AI_BASH%" "%~dp0ai" %*
