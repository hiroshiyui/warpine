#define INCL_WIN
#include <os2.h>

int main( void )
{
    HAB hab;
    HMQ hmq;

    hab = WinInitialize( 0 );
    if( hab == 0 ) return( 0 );

    hmq = WinCreateMsgQueue( hab, 0 );
    if( hmq == 0 ) {
        WinTerminate( hab );
        return( 0 );
    }

    WinMessageBox( HWND_DESKTOP,
                   HWND_DESKTOP,
                   "Hello from Warpine PM Environment!",
                   "Phase 3 Initialization",
                   0,
                   MB_OK | MB_INFORMATION );

    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );

    return( 0 );
}
