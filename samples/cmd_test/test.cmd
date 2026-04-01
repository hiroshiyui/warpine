:: Warpine CMD.EXE built-in commands test script
:: Run with: warpine CMD.EXE /C test.cmd

REM Test 1: VER
echo === Test: VER ===
ver

REM Test 2: ECHO
echo === Test: ECHO ===
echo Hello from CMD test script!

REM Test 3: SET
echo === Test: SET ===
set WARPINE_TEST=yes
set WARPINE_TEST

REM Test 4: CD and directory operations
echo === Test: MD/CD/RD ===
md testdir
cd testdir
cd
cd ..
rd testdir

REM Test 5: DIR
echo === Test: DIR ===
dir

REM Test 6: TYPE
echo === Test: TYPE ===
echo This is a test file. > testfile.txt
type testfile.txt
del testfile.txt

REM Test 7: HELP
echo === Test: HELP ===
help

REM Test 8: CLS
echo === Test: CLS (screen clear) ===
cls
echo Screen cleared.

REM Test 9: I/O Redirection
echo === Test: I/O Redirection ===
echo Redirect test > redirect_out.txt
type redirect_out.txt
del redirect_out.txt
echo Append line 1 > append_test.txt
echo Append line 2 >> append_test.txt
type append_test.txt
del append_test.txt

REM Test 10: Pipe
echo === Test: Pipe ===
echo Pipe test | echo Pipe output:

echo === All tests complete ===
exit 0
