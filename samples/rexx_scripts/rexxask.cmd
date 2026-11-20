/* SPDX-License-Identifier: GPL-3.0-or-later */
/* rexxask.cmd - PULL and PARSE PULL read the shell's input when the
   queue is empty (a pipe here); a syntax error at the end shows how REXX
   reports one. */
'@echo off'
pull upper
parse pull exact
say "upper:" upper
say "exact:" exact
say 1 + "not a number"
