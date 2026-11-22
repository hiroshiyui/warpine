/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_pointer_test.c - PM pointers under Warpine
 *
 *   - WinQuerySysPointer: stable system pointer handles, copies (fCopy)
 *     that WinDestroyPointer frees, PMERR_PARAMETER_OUT_OF_RANGE
 *   - WinSetPointer / WinQueryPointer, NULLHANDLE removing the pointer,
 *     PMERR_INVALID_HPTR for a handle that is no pointer
 *   - WinShowPointer: the pointer display level (SV_POINTERLEVEL), never
 *     below zero
 *   - WinLoadPointer: a black-and-white pointer and a bitmap array from
 *     RT_POINTER resources (written by make_ptr.py), a new handle on every
 *     call, PMERR_RESOURCE_NOT_FOUND
 *   - WinDestroyPointer: not for system pointers, not twice
 *   - WinDefWindowProc sets the arrow on WM_MOUSEMOVE, a sizing arrow over
 *     a frame's sizing border
 *   - controls: the I-beam over an entry field, the arrow over a button,
 *     and WM_CONTROLPOINTER, which asks the owner (the client) and shows
 *     the pointer it returns; WinDefWindowProc returns the proposal
 *
 * Each check is written to stdout:
 *
 *   pm_pointer_test: <name>: ok
 *   pm_pointer_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_pointer_test: done <passed>/<total>".
 */
#define INCL_WIN
#define INCL_WINPOINTERS
#include <os2.h>
#include <string.h>
#include "pm_pointer_test.h"

#define BAD_HPTR  ( (HPOINTER)0x1234 )
#define ID_ENTRY  10
#define ID_BUTTON 11

static HPOINTER answer = NULLHANDLE;   /* what WM_CONTROLPOINTER returns */
static ULONG    asked_id = 0;
static HPOINTER asked_ptr = NULLHANDLE;

static int n_checks = 0;
static int n_passed = 0;

/* ---------- output (no libc printf) ---------- */

static char *append_str( char *p, const char *s )
{
    while( *s ) *p++ = *s++;
    *p = '\0';
    return p;
}

static char *append_long( char *p, LONG n )
{
    char  tmp[12];
    int   i = 0;
    ULONG u = n < 0 ? (ULONG)-n : (ULONG)n;
    if( n < 0 ) *p++ = '-';
    do { tmp[i++] = (char)( '0' + u % 10 ); u /= 10; } while( u );
    while( i ) *p++ = tmp[--i];
    *p = '\0';
    return p;
}

static void write_stdout( const char *s )
{
    ULONG written;
    DosWrite( 1, (PVOID)s, strlen( s ), &written );
}

static void check( const char *what, LONG got, LONG want )
{
    char  line[160];
    char *p = append_str( line, "pm_pointer_test: " );
    p = append_str( p, what );
    if( got == want ) {
        append_str( p, ": ok\r\n" );
        n_passed++;
    } else {
        p = append_str( p, ": FAIL got " );
        p = append_long( p, got );
        p = append_str( p, " want " );
        p = append_long( p, want );
        append_str( p, "\r\n" );
    }
    write_stdout( line );
    n_checks++;
}

static USHORT last_error( HAB hab )
{
    return ERRORIDERROR( WinGetLastError( hab ) );
}

static MRESULT EXPENTRY ClientWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    if( msg == WM_CONTROLPOINTER ) {
        asked_id = SHORT1FROMMP( mp1 );
        asked_ptr = (HPOINTER)mp2;
        if( answer != NULLHANDLE )
            return (MRESULT)answer;
    }
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

int main( void )
{
    HAB      hab;
    HMQ      hmq;
    HWND     frame, client;
    ULONG    flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_SIZEBORDER;
    HWND     entry, button;
    HPOINTER arrow, text, wait, copy, cross, cross2, array;
    char     line[64], *p;

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "PointerTest", ClientWndProc, 0, 0 );
    frame = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &flags, "PointerTest",
                                "PM Pointer Test", 0, NULLHANDLE, 0, &client );

    /* System pointers. */
    arrow = WinQuerySysPointer( HWND_DESKTOP, SPTR_ARROW, FALSE );
    check( "system arrow has a handle", arrow != NULLHANDLE, TRUE );
    check( "system arrow handle is stable",
           WinQuerySysPointer( HWND_DESKTOP, SPTR_ARROW, FALSE ) == arrow, TRUE );
    text = WinQuerySysPointer( HWND_DESKTOP, SPTR_TEXT, FALSE );
    wait = WinQuerySysPointer( HWND_DESKTOP, SPTR_WAIT, FALSE );
    check( "I-beam differs from the arrow", text != NULLHANDLE && text != arrow, TRUE );
    check( "wait differs from the I-beam", wait != NULLHANDLE && wait != text, TRUE );
    check( "every SPTR_ size/move/illegal pointer has a handle",
           WinQuerySysPointer( HWND_DESKTOP, SPTR_SIZENWSE, FALSE ) != NULLHANDLE
           && WinQuerySysPointer( HWND_DESKTOP, SPTR_SIZENESW, FALSE ) != NULLHANDLE
           && WinQuerySysPointer( HWND_DESKTOP, SPTR_SIZEWE, FALSE ) != NULLHANDLE
           && WinQuerySysPointer( HWND_DESKTOP, SPTR_SIZENS, FALSE ) != NULLHANDLE
           && WinQuerySysPointer( HWND_DESKTOP, SPTR_MOVE, FALSE ) != NULLHANDLE
           && WinQuerySysPointer( HWND_DESKTOP, SPTR_ILLEGAL, FALSE ) != NULLHANDLE, TRUE );
    check( "message-box icon pointer has a handle",
           WinQuerySysPointer( HWND_DESKTOP, SPTR_ICONWARNING, FALSE ) != NULLHANDLE, TRUE );
    check( "iptr 0 fails", WinQuerySysPointer( HWND_DESKTOP, 0, FALSE ), NULLHANDLE );
    check( "iptr 0 -> error code", last_error( hab ), PMERR_PARAMETER_OUT_OF_RANGE );

    /* Copies. */
    copy = WinQuerySysPointer( HWND_DESKTOP, SPTR_WAIT, TRUE );
    check( "a copy is a new handle", copy != NULLHANDLE && copy != wait, TRUE );
    check( "a copy can be set", WinSetPointer( HWND_DESKTOP, copy ), TRUE );
    check( "WinDestroyPointer frees the copy", WinDestroyPointer( copy ), TRUE );
    check( "a copy is destroyed once", WinDestroyPointer( copy ), FALSE );
    check( "destroyed twice -> error code", last_error( hab ), PMERR_INVALID_HPTR );
    check( "the system arrow cannot be destroyed", WinDestroyPointer( arrow ), FALSE );

    /* Setting and querying. */
    check( "WinSetPointer wait", WinSetPointer( HWND_DESKTOP, wait ), TRUE );
    check( "WinQueryPointer reports wait", WinQueryPointer( HWND_DESKTOP ) == wait, TRUE );
    check( "WinSetPointer bad handle", WinSetPointer( HWND_DESKTOP, BAD_HPTR ), FALSE );
    check( "bad handle -> error code", last_error( hab ), PMERR_INVALID_HPTR );
    check( "a failed set keeps the pointer", WinQueryPointer( HWND_DESKTOP ) == wait, TRUE );
    check( "WinSetPointer NULLHANDLE", WinSetPointer( HWND_DESKTOP, NULLHANDLE ), TRUE );
    check( "no pointer after NULLHANDLE", WinQueryPointer( HWND_DESKTOP ), NULLHANDLE );
    check( "WinSetPointer I-beam", WinSetPointer( HWND_DESKTOP, text ), TRUE );

    /* The pointer display level. */
    check( "pointer level starts at 0", WinQuerySysValue( HWND_DESKTOP, SV_POINTERLEVEL ), 0 );
    check( "WinShowPointer FALSE", WinShowPointer( HWND_DESKTOP, FALSE ), TRUE );
    WinShowPointer( HWND_DESKTOP, FALSE );
    check( "two hides -> level 2", WinQuerySysValue( HWND_DESKTOP, SV_POINTERLEVEL ), 2 );
    WinShowPointer( HWND_DESKTOP, TRUE );
    WinShowPointer( HWND_DESKTOP, TRUE );
    check( "WinShowPointer TRUE at level 0", WinShowPointer( HWND_DESKTOP, TRUE ), TRUE );
    check( "level never below 0", WinQuerySysValue( HWND_DESKTOP, SV_POINTERLEVEL ), 0 );
    check( "SV_CXPOINTER", WinQuerySysValue( HWND_DESKTOP, SV_CXPOINTER ), 32 );
    check( "SV_MOUSEPRESENT", WinQuerySysValue( HWND_DESKTOP, SV_MOUSEPRESENT ), TRUE );

    /* Pointers from resources. */
    cross = WinLoadPointer( HWND_DESKTOP, NULLHANDLE, ID_PTR_CROSS );
    check( "WinLoadPointer black-and-white pointer", cross != NULLHANDLE, TRUE );
    cross2 = WinLoadPointer( HWND_DESKTOP, NULLHANDLE, ID_PTR_CROSS );
    check( "WinLoadPointer makes a new copy each time",
           cross2 != NULLHANDLE && cross2 != cross, TRUE );
    array = WinLoadPointer( HWND_DESKTOP, NULLHANDLE, ID_PTR_ARRAY );
    check( "WinLoadPointer bitmap array", array != NULLHANDLE, TRUE );
    check( "WinLoadPointer missing id", WinLoadPointer( HWND_DESKTOP, NULLHANDLE, ID_PTR_NONE ),
           NULLHANDLE );
    check( "missing id -> error code", last_error( hab ), PMERR_RESOURCE_NOT_FOUND );
    check( "WinSetPointer loaded pointer", WinSetPointer( HWND_DESKTOP, array ), TRUE );
    check( "WinQueryPointer reports it", WinQueryPointer( HWND_DESKTOP ) == array, TRUE );
    check( "WinSetPointer cross", WinSetPointer( HWND_DESKTOP, cross ), TRUE );

    /* The default window procedure shows the arrow on mouse moves. */
    WinDefWindowProc( client, WM_MOUSEMOVE, MPFROM2SHORT( 10, 10 ), 0 );
    check( "WinDefWindowProc(WM_MOUSEMOVE) sets the arrow",
           WinQueryPointer( HWND_DESKTOP ) == arrow, TRUE );

    /* A frame's sizing border shows the sizing arrows. */
    WinDefWindowProc( frame, WM_MOUSEMOVE, MPFROM2SHORT( 1, 200 ), 0 );
    check( "left sizing border: SPTR_SIZEWE",
           WinQueryPointer( HWND_DESKTOP ) == WinQuerySysPointer( HWND_DESKTOP, SPTR_SIZEWE, FALSE ), TRUE );
    WinDefWindowProc( frame, WM_MOUSEMOVE, MPFROM2SHORT( 1, 1 ), 0 );
    check( "bottom-left corner: SPTR_SIZENESW",
           WinQueryPointer( HWND_DESKTOP ) == WinQuerySysPointer( HWND_DESKTOP, SPTR_SIZENESW, FALSE ), TRUE );

    /* Controls show their own pointer, which their owner may change. */
    entry = WinCreateWindow( client, WC_ENTRYFIELD, "text", WS_VISIBLE, 10, 10, 100, 20,
                             client, HWND_TOP, ID_ENTRY, NULL, NULL );
    button = WinCreateWindow( client, WC_BUTTON, "OK", WS_VISIBLE | BS_PUSHBUTTON, 10, 40, 60, 24,
                              client, HWND_TOP, ID_BUTTON, NULL, NULL );
    WinSendMsg( entry, WM_MOUSEMOVE, MPFROM2SHORT( 5, 5 ), 0 );
    check( "the I-beam over an entry field", WinQueryPointer( HWND_DESKTOP ) == text, TRUE );
    check( "WM_CONTROLPOINTER mp1 = the control's id", asked_id, ID_ENTRY );
    check( "WM_CONTROLPOINTER mp2 = the I-beam", asked_ptr == text, TRUE );
    check( "WinDefWindowProc(WM_CONTROLPOINTER) returns mp2",
           (HPOINTER)WinDefWindowProc( client, WM_CONTROLPOINTER, MPFROMSHORT( ID_ENTRY ), (MPARAM)wait ) == wait, TRUE );
    answer = wait;
    WinSendMsg( entry, WM_MOUSEMOVE, MPFROM2SHORT( 6, 5 ), 0 );
    check( "the owner's pointer is shown", WinQueryPointer( HWND_DESKTOP ) == wait, TRUE );
    answer = NULLHANDLE;
    WinSendMsg( button, WM_MOUSEMOVE, MPFROM2SHORT( 5, 5 ), 0 );
    check( "the arrow over a button", WinQueryPointer( HWND_DESKTOP ) == arrow, TRUE );
    check( "WM_CONTROLPOINTER from the button", asked_id, ID_BUTTON );

    check( "WinDestroyPointer cross", WinDestroyPointer( cross ), TRUE );
    check( "WinDestroyPointer second cross", WinDestroyPointer( cross2 ), TRUE );
    check( "WinDestroyPointer array", WinDestroyPointer( array ), TRUE );
    check( "a destroyed pointer cannot be set", WinSetPointer( HWND_DESKTOP, cross ), FALSE );

    p = append_str( line, "pm_pointer_test: done " );
    p = append_long( p, n_passed );
    p = append_str( p, "/" );
    p = append_long( p, n_checks );
    append_str( p, "\r\n" );
    write_stdout( line );

    WinDestroyWindow( frame );
    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return 0;
}
