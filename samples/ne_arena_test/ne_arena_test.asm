; ne_arena_test.asm — a 16-bit program cannot reach the shared arena
; (ADR-0049 addendum)
;
; SPDX-License-Identifier: GPL-3.0-or-later
;
; The session's shared arena starts at linear 0x0E000000. A 16-bit program
; can address any 64 KB of memory through the GDT's data tiles (one per
; 64 KB, selector 0x30 + 8 * n), so it could reach the arena's first block
; through tile 0xE00. Paging keeps it out: the arena's pages are present
; only for the blocks the process holds, and a 16-bit program holds none.
;
; This program reads a word through its own data segment (a positive
; control), then through the arena's tile. The second read must trap and
; end the program; if it goes on, it says so.
;
; Output: "ne_arena_test: own data read: ok", "ne_arena_test info: reading
; the arena", and only when the arena is reachable
; "ne_arena_test: read the arena: FAIL".
;
; No C runtime (see ne_hello.asm); OS/2 Pascal calls.

        .286

EXTRN   DosWrite : FAR
EXTRN   DosExit  : FAR

; GDT data tile of linear 0x0E000000 (SHARED_ARENA_BASE / TILE_SIZE = 0xE00),
; at ring 3: (TILED_SEL_START_INDEX + 0xE00) * 8 + 3.
ARENA_TILE EQU  ((6 + 0E00h) * 8) + 3

_DATA   SEGMENT WORD PUBLIC 'DATA'
marker     DW   5A5Ah
own_ok     DB   "ne_arena_test: own data read: ok", 0DH, 0AH
OWN_OK_LEN EQU  $ - own_ok
own_bad    DB   "ne_arena_test: own data read: FAIL", 0DH, 0AH
OWN_BAD_LEN EQU $ - own_bad
probe_msg  DB   "ne_arena_test info: reading the arena", 0DH, 0AH
PROBE_LEN  EQU  $ - probe_msg
fail_msg   DB   "ne_arena_test: read the arena: FAIL", 0DH, 0AH
FAIL_LEN   EQU  $ - fail_msg
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
        mov     ax, marker
        cmp     ax, 5A5Ah
        jne     own_failed
        WRITE   own_ok, OWN_OK_LEN
        jmp     short probe
own_failed:
        WRITE   own_bad, OWN_BAD_LEN

probe:
        WRITE   probe_msg, PROBE_LEN
        mov     ax, ARENA_TILE
        mov     es, ax
        mov     ax, es:[0]
        WRITE   fail_msg, FAIL_LEN

        push    1                       ; EXIT_PROCESS
        push    0                       ; exit code
        call    DosExit
        hlt

_TEXT   ENDS

        END     start
