#define INCL_DOS
#include <os2.h>
#include <stdio.h>

void _Optlink child_thread(ULONG param) {
    HEV hev = (HEV)param;
    ULONG written;
    APIRET rc;
    
    DosWrite(1, "Child: Posting semaphore in 2 seconds...\r\n", 42, &written);
    DosSleep(2000);
    
    rc = DosPostEventSem(hev);
    if (rc == 0) {
        DosWrite(1, "Child: Semaphore posted.\r\n", 26, &written);
    } else {
        DosWrite(1, "Child: DosPostEventSem failed!\r\n", 32, &written);
    }
}

int main(void) {
    HEV hev;
    TID tid;
    APIRET rc;
    ULONG written;

    /* DosCreateEventSem(PSZ pszName, PHEV phev, ULONG flAttr, BOOL fState) */
    rc = DosCreateEventSem(NULL, &hev, 0, FALSE);
    if (rc != 0) {
        DosWrite(1, "Main: DosCreateEventSem failed!\r\n", 33, &written);
        return 1;
    }

    DosWrite(1, "Main: Created event semaphore.\r\n", 32, &written);

    rc = DosCreateThread(&tid, (PFNTHREAD)child_thread, (ULONG)hev, 0, 8192);
    if (rc != 0) {
        DosWrite(1, "Main: DosCreateThread failed!\r\n", 31, &written);
        return 1;
    }

    DosWrite(1, "Main: Waiting for semaphore...\r\n", 32, &written);
    
    /* DosWaitEventSem(HEV hev, ULONG msec) */
    rc = DosWaitEventSem(hev, SEM_INDEFINITE_WAIT);
    if (rc == 0) {
        DosWrite(1, "Main: Semaphore signaled! Closing...\r\n", 38, &written);
    } else {
        DosWrite(1, "Main: DosWaitEventSem failed!\r\n", 31, &written);
    }

    DosCloseEventSem(hev);
    DosExit(1, 0);
    return 0;
}
