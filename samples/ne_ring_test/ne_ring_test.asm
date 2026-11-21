; ne_ring_test.asm — the ring a 16-bit OS/2 program runs at (ADR-0113)
;
; SPDX-License-Identifier: GPL-3.0-or-later
;
; Warpine runs 16-bit programs at ring 3, as OS/2 does (WARPINE_RING=0:
; ring 0, for one release). This program:
;   - prints the ring its code runs at (CS's RPL) as information;
;   - checks that its data and stack selectors carry the same RPL;
;   - executes CLI, which only ring 0 (or an I/O privilege level the
;     program does not have) may execute. At ring 3 the processor refuses
;     it and, with no handler, the program ends; at ring 0 it goes on and
;     says so, then ends with DosExit(EXIT_PROCESS, 0).
;
; Output: "ne_ring_test info: running at ring <n>", then
; "ne_ring_test: data and stack at the code's ring: ok" (or FAIL), then
; "ne_ring_test info: executing CLI", and at ring 0
; "ne_ring_test info: went on after CLI".
;
; No C runtime (see ne_hello.asm); OS/2 Pascal calls.

        .286

EXTRN   DosWrite : FAR
EXTRN   DosExit  : FAR

RPL_MASK EQU    3

_DATA   SEGMENT WORD PUBLIC 'DATA'
ring_msg   DB   "ne_ring_test info: running at ring "
ring_digit DB   "?", 0DH, 0AH
RING_LEN   EQU  $ - ring_msg
ok_msg     DB   "ne_ring_test: data and stack at the code's ring: ok", 0DH, 0AH
OK_LEN     EQU  $ - ok_msg
fail_msg   DB   "ne_ring_test: data and stack at the code's ring: FAIL", 0DH, 0AH
FAIL_LEN   EQU  $ - fail_msg
cli_msg    DB   "ne_ring_test info: executing CLI", 0DH, 0AH
CLI_LEN    EQU  $ - cli_msg
after_msg  DB   "ne_ring_test info: went on after CLI", 0DH, 0AH
AFTER_LEN  EQU  $ - after_msg
written    DW   0
_DATA   ENDS

_STACK  SEGMENT PARA STACK 'STACK'
        DB      1024 DUP (0)
_STACK  ENDS

; DosWrite(1, ds:msg, len, ds:written)
WRITE   MACRO   msg, len
        push    1
        push    ds
        push    OFFSET msg
        push    len
        push    ds
        push    OFFSET written
        call    DosWrite
        ENDM

_TEXT   SEGMENT BYTE PUBLIC 'CODE'
        ASSUME  CS:_TEXT, DS:_DATA

start:
        mov     ax, cs
        and     ax, RPL_MASK
        mov     si, ax                  ; the ring, kept across the calls
        add     al, '0'
        mov     ring_digit, al
        WRITE   ring_msg, RING_LEN

        mov     ax, ds
        and     ax, RPL_MASK
        cmp     ax, si
        jne     bad
        mov     ax, ss
        and     ax, RPL_MASK
        cmp     ax, si
        jne     bad
        WRITE   ok_msg, OK_LEN
        jmp     short privileged
bad:
        WRITE   fail_msg, FAIL_LEN

privileged:
        WRITE   cli_msg, CLI_LEN
        cli
        WRITE   after_msg, AFTER_LEN

        push    1                       ; EXIT_PROCESS
        push    0                       ; exit code
        call    DosExit
        hlt

_TEXT   ENDS

        END     start
