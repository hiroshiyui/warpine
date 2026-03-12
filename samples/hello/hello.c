#define INCL_DOS
#include <os2.h>

int main(void) {
    char *msg = "Hello from Warpine OS/2 Environment!\r\n";
    ULONG written;

    // DosWrite(HFILE hFile, PVOID pBuf, ULONG cbBuf, PULONG pcbActual)
    // hFile = 1 for stdout
    DosWrite(1, msg, 40, &written);

    // DosExit(ULONG action, ULONG result)
    // action = 1 for EXIT_PROCESS
    DosExit(1, 0);

    return 0; // Should not reach here
}
