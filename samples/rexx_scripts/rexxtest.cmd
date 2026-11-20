/* SPDX-License-Identifier: GPL-3.0-or-later */
/* rexxtest.cmd - REXX in the built-in CMD.EXE (ADR-0083), R1's language
   core: run as REXXTEST.CMD alpha beta. TOOL.EXE is samples/cmd_tool,
   REXXHELP.CMD an external routine. Each check writes "ok: name" or
   "FAIL: name", then "rexxtest: done <passed>/<total>". */
'@echo off'
passed = 0; total = 0
parse arg first second
call check "arguments", first second == "alpha beta"
parse source . how name
call check "parse source", how name == "COMMAND C:\REXXTEST.CMD"
call check "arithmetic", 1 + 2 * 3 = 7 & 7 / 2 = 3.5 & 7 // 2 = 1 & 2 ** 10 = 1024
numeric digits 30
call check "numeric digits", 1 / 3 == "0.333333333333333333333333333333"
numeric digits
call check "strings", substr("warpine", 2, 3) || reverse("ab") == "arpba"
call check "words", word("a b c", 2) words("a  b c") == "b 3"
total_loop = 0
do i = 1 to 10 by 2; total_loop = total_loop + i; end
call check "do loop", total_loop = 25
s. = 0; s.1 = 5; s.2 = 7; k = 2
call check "stems", s.1 + s.k + s.9 = 12
call check "procedure", fact(10) = 3628800
parse value "key=value;rest" with key "=" val ";" tail
call check "parse templates", key val tail == "key value rest"
select
  when total_loop > 100 then x = "big"
  when total_loop > 10 then x = "medium"
  otherwise x = "small"
end
call check "select", x == "medium"
interpret "y = 6 * 7"
call check "interpret", y = 42
push "second"; queue "third"; push "first"
pull a; pull b; pull c
call check "queue", a b c == "FIRST SECOND THIRD"
"TOOL exit 9"
call check "command rc", rc = 9
call check "external routine", rexxhelp(14) = 42
signal skip
call check "signal", 0
skip:
call check "signal", 1
say "rexxtest: done" passed"/"total
exit total - passed

check: procedure expose passed total
  parse arg what, good
  total = total + 1
  if good then do; passed = passed + 1; say "ok:" what; end
  else say "FAIL:" what
  return

fact: procedure
  arg n
  if n < 2 then return 1
  return n * fact(n - 1)
