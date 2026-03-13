#define INCL_DOS
#include <os2.h>
#include <stdio.h>

int main(void) {
    HDIR hdir = HDIR_CREATE;
    FILEFINDBUF3 findbuf;
    ULONG count = 1;
    APIRET rc;

    printf("Searching for samples/*.c...\n");
    rc = DosFindFirst("samples/*.c", &hdir, FILE_NORMAL, &findbuf, sizeof(findbuf), &count, FIL_STANDARD);
    
    while (rc == 0 && count > 0) {
        printf("Found: %s (size=%lu)\n", findbuf.achName, findbuf.cbFile);
        fflush(stdout);
        count = 1;
        rc = DosFindNext(hdir, &findbuf, sizeof(findbuf), &count);
    }

    DosFindClose(hdir);
    return 0;
}
