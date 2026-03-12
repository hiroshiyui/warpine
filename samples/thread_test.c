#define INCL_DOS
#include <os2.h>

void _Optlink thread_func(ULONG param) {
    ULONG written;
    DosWrite(1, "Hello from child thread!\r\n", 26, &written);
    DosSleep(100);
}

int main(void) {
    TID tid;
    APIRET rc;
    ULONG written;
    
    DosWrite(1, "Main thread: Creating child...\r\n", 32, &written);
    rc = DosCreateThread(&tid, (PFNTHREAD)thread_func, 0, 0, 8192);
    if (rc == 0) {
        DosWrite(1, "Main thread: Waiting for child...\r\n", 34, &written);
        DosWaitThread(&tid, 0);
        DosWrite(1, "Main thread: Child finished.\r\n", 30, &written);
    } else {
        DosWrite(1, "Main thread: CreateThread failed.\r\n", 35, &written);
    }
    
    DosExit(1, 0);
    return 0;
}
