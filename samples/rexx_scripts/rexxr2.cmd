/* SPDX-License-Identifier: GPL-3.0-or-later */
/* rexxr2.cmd - REXX in the built-in CMD.EXE, phase R2 (ADR-0084):
   conditions, streams, REXXUTIL, NUMERIC FORM ENGINEERING and TRACE.
   Run as REXXR2.CMD; TOOL.EXE is samples/cmd_tool. Each check writes
   "ok: name" or "FAIL: name", then "rexxr2: done <passed>/<total>". */
'@echo off'
passed = 0; total = 0
if RxFuncQuery('SysLoadFuncs') then do
  call RxFuncAdd 'SysLoadFuncs', 'RexxUtil', 'SysLoadFuncs'
  call SysLoadFuncs
end

/* Conditions */
signal on syntax name bad_math
x = 1 + 'one'
call check "signal on syntax", 0
bad_math:
call check "signal on syntax", rc = 41 & condition('C') == 'SYNTAX'
signal on novalue
y = undefined_variable
call check "signal on novalue", 0
novalue:
call check "signal on novalue", condition('D') == 'UNDEFINED_VARIABLE'
signal off novalue
errors = 0
call on error
'TOOL exit 3'
call check "call on error", errors = 1 & rc = 3
call off error

/* Streams */
file = 'R2DATA.TXT'
call SysFileDelete file
do i = 1 to 3; call lineout file, 'line' i; end
call lineout file
call check "lineout and lines", lines(file, 'C') = 3
got = ''
do while lines(file) > 0; got = got linein(file); end
call check "linein", space(got) == 'line 1 line 2 line 3'
call check "stream query", stream(file, 'C', 'QUERY SIZE') = 24 & stream(file, 'C', 'QUERY EXISTS') == 'C:\R2DATA.TXT'
call check "charin", charin(file, 1, 4) == 'line'
call stream file, 'C', 'CLOSE'
signal on notready
x = linein('NO_SUCH.TXT')
call check "notready", 0
notready:
call check "notready", condition('D') == 'NO_SUCH.TXT'
signal off notready

/* REXXUTIL */
call check "sysos2ver", SysOS2Ver() = 2.45
call SysMkDir 'R2DIR'
call lineout 'R2DIR\NOTE.TXT', 'find the needle here'
call lineout 'R2DIR\NOTE.TXT'
call SysFileTree 'R2DIR\*', 'files', 'FO'
call check "sysfiletree", files.0 = 1 & files.1 == 'C:\R2DIR\NOTE.TXT'
call SysFileSearch 'NEEDLE', 'R2DIR\NOTE.TXT', 'hits'
call check "sysfilesearch", hits.0 = 1 & hits.1 == 'find the needle here'
call check "sysfiledelete", SysFileDelete('R2DIR\NOTE.TXT') = 0 & SysRmDir('R2DIR') = 0 & SysFileDelete(file) = 0

/* NUMERIC FORM and TRACE */
numeric form engineering
call check "numeric form", 1.5E+10 * 1 == '15E+9'
numeric form scientific
trace r
z = 6 * 7
trace off
call check "trace", z = 42

say "rexxr2: done" passed"/"total
exit total - passed

error:
  errors = errors + 1
  return

check: procedure expose passed total
  parse arg what, good
  total = total + 1
  if good then do; passed = passed + 1; say "ok:" what; end
  else say "FAIL:" what
  return
