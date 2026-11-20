@ECHO OFF
REM SPDX-License-Identifier: GPL-3.0-or-later
REM session.cmd - the built-in CMD.EXE's PATH, DETACH and PROMPT (ADR-0080).
REM TOOL.EXE is samples/cmd_tool.
MD BIN
MOVE TOOL.EXE BIN
PATH ;
TOOL exit 0 || ECHO not-on-path
PATH C:\BIN
TOOL exit 0 && ECHO found-on-path
PATH
DETACH TOOL touch DONE.TXT
PROMPT $p$g
ECHO ON
ECHO prompt-shown
