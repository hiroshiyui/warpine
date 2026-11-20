@ECHO OFF
REM SPDX-License-Identifier: GPL-3.0-or-later
REM batch.cmd - the built-in CMD.EXE's batch control (ADR-0078): %0-%9 and
REM SHIFT, IF ERRORLEVEL / EXIST / ==, FOR, CALL, GOTO, SETLOCAL / ENDLOCAL,
REM PAUSE and EXIT. Run as BATCH.CMD first second; TOOL.EXE is samples/cmd_tool.
SETLOCAL
ECHO args: %0 %1 %2
SHIFT
ECHO shifted: %1
TOOL exit 4
IF ERRORLEVEL 4 ECHO errorlevel-4
IF NOT ERRORLEVEL 5 ECHO below-5
IF EXIST TOOL.EXE ECHO tool-exists
IF "%1"=="second" ECHO second-arg
FOR %%N IN (one two three) DO ECHO item-%%N
CALL BATCHSUB.CMD called
ECHO back
GOTO SKIP
ECHO never
:SKIP
SET LOCALVAR=yes
ENDLOCAL
ECHO [%LOCALVAR%]
PAUSE
ECHO paused
EXIT 6
ECHO not-after-exit
