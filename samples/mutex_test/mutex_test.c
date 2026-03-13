#define INCL_DOS
#include <os2.h>
#include <stdio.h>

void _Optlink child_thread(ULONG param) {
    HMTX hmtx = (HMTX)param;
    ULONG written;
    APIRET rc;
    
    DosWrite(1, "Child: Requesting mutex...\r\n", 28, &written);
    rc = DosRequestMutexSem(hmtx, SEM_INDEFINITE_WAIT);
    if (rc == 0) {
        DosWrite(1, "Child: Got mutex! Holding for 1 second...\r\n", 43, &written);
        DosSleep(1000);
        DosWrite(1, "Child: Releasing mutex.\r\n", 25, &written);
        DosReleaseMutexSem(hmtx);
    } else {
        DosWrite(1, "Child: DosRequestMutexSem failed!\r\n", 35, &written);
    }
}

int main(void) {
    HMTX hmtx;
    TID tid;
    APIRET rc;
    ULONG written;

    /* DosCreateMutexSem(PSZ pszName, PHMTX phmtx, ULONG flAttr, BOOL fState) */
    rc = DosCreateMutexSem(NULL, &hmtx, 0, FALSE);
    if (rc != 0) {
        DosWrite(1, "Main: DosCreateMutexSem failed!\r\n", 33, &written);
        return 1;
    }

    DosWrite(1, "Main: Created mutex. Testing recursive lock...\r\n", 48, &written);
    DosRequestMutexSem(hmtx, SEM_INDEFINITE_WAIT);
    DosRequestMutexSem(hmtx, SEM_INDEFINITE_WAIT);
    DosWrite(1, "Main: Recursive locks OK. Releasing once...\r\n", 45, &written);
    DosReleaseMutexSem(hmtx);
    
    DosWrite(1, "Main: Creating child thread...\r\n", 32, &written);
    rc = DosCreateThread(&tid, (PFNTHREAD)child_thread, (ULONG)hmtx, 0, 8192);
    if (rc != 0) {
        DosWrite(1, "Main: DosCreateThread failed!\r\n", 31, &written);
        return 1;
    }

    DosSleep(500);
    DosWrite(1, "Main: Releasing mutex for child...\r\n", 36, &written);
    DosReleaseMutexSem(hmtx);
    
    DosWaitThread(&tid, 0);
    DosWrite(1, "Main: Child finished. Closing mutex.\r\n", 38, &written);
    DosCloseMutexSem(hmtx);
    
    DosExit(1, 0);
    return 0;
}
