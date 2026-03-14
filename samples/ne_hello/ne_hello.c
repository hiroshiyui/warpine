/*
 * ne_hello.c — Minimal 16-bit OS/2 NE format test program.
 *
 * This is a 16-bit OS/2 1.x application compiled to the NE (New Executable)
 * format. It exercises:
 * - DosWrite to stdout (handle 1)
 * - DosExit with exit code 0
 *
 * Used to verify warpine's NE format parser and (eventually) 16-bit loader.
 */

#define INCL_DOS
#include <os2.h>

int main(void) {
    USHORT written;
    DosWrite(1, "Hello from NE (16-bit OS/2)!\r\n", 30, &written);
    DosExit(EXIT_PROCESS, 0);
    return 0;
}
