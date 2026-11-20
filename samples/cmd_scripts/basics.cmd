REM SPDX-License-Identifier: GPL-3.0-or-later
REM basics.cmd - the built-in CMD.EXE's foundations (ADR-0077): variables,
REM &, && and ||, programs reading pipes, the current directory passed to
REM programs, and return codes. TOOL.EXE is samples/cmd_tool.
SET GREETING=hello
ECHO [%GREETING%] [%NO_SUCH_VARIABLE%] 100%%
TOOL exit 3 && ECHO and-ran || ECHO or-ran
TOOL exit 0 && ECHO and-after-0
ECHO piped | TOOL count
MD WORK & CD WORK & TOOL cwd & CD \
ECHO a^&b
TOOL exit 5
