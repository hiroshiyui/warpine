; ne_hello.asm — Minimal 16-bit OS/2 NE format hello world
;
; No Watcom C runtime. Directly calls DosWrite then DosExit.
; Three segments: CODE (seg 1), DATA (seg 2), STACK (seg 3).
; The NE loader pre-loads:
;   DS = auto_data_segment = 2  (the DATA segment tile)
;   SS:SP from NE header SS:SP fields = seg 3 : 0x400
; so we do not need to set up DS or SS ourselves.
;
; Calling convention: OS/2 Pascal (left-to-right push, callee cleans).
; For far pointers: push segment word first, then offset word.
; Stack after CALL FAR (from low address to high address):
;   [ret_IP][ret_CS][last_arg ... first_arg]
;
; DosWrite(hf:16, pvBuf:far, cbBuf:16, pcbBytesWritten:far)  ordinal 138
;   push hf, push pvBuf_seg, push pvBuf_off, push cbBuf,
;   push pcbWritten_seg, push pcbWritten_off
;   → total 12 arg bytes (ne_api_arg_bytes = 12)
;
; DosExit(fTerminate:16, usExitCode:16)                       ordinal 5
;   push fTerminate, push usExitCode
;   → total 4 arg bytes (ne_api_arg_bytes = 4)

        .286

EXTRN   DosWrite : FAR
EXTRN   DosExit  : FAR

_DATA   SEGMENT WORD PUBLIC 'DATA'
msg     DB      "Hello from NE (16-bit OS/2)!", 0DH, 0AH  ; 30 bytes
written DW      0
_DATA   ENDS

_STACK  SEGMENT PARA STACK 'STACK'
        DB      1024 DUP (0)
_STACK  ENDS

_TEXT   SEGMENT BYTE PUBLIC 'CODE'
        ASSUME  CS:_TEXT, DS:_DATA

start:
        ; DS is pre-loaded to the DATA segment tile by the NE loader.
        ; SS:SP is pre-loaded to the STACK segment tile, SP=1024.

        ; DosWrite(hf=1, pvBuf=msg, cbBuf=30, pcbBytesWritten=&written)
        ; Pascal: push left to right; for far ptr push seg then off.
        push    1                       ; hf = stdout (handle 1)
        push    ds                      ; pvBuf segment
        push    OFFSET msg              ; pvBuf offset (= 0)
        push    30                      ; cbBuf
        push    ds                      ; pcbBytesWritten segment
        push    OFFSET written          ; pcbBytesWritten offset (= 30)
        call    DosWrite

        ; DosExit(fTerminate=EXIT_PROCESS=1, usExitCode=0)
        push    1                       ; fTerminate
        push    0                       ; usExitCode
        call    DosExit

        ; Should never reach here
        hlt

_TEXT   ENDS

        END     start
