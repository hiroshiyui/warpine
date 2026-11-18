/*
 * pm_msg_hook_test.c - PM message calls and hooks under Warpine
 *
 * Exercises:
 *
 *   - WinSetHook / WinReleaseHook for the calling queue (HMQ_CURRENT)
 *   - HK_INPUT: called by WinGetMsg / WinPeekMsg with the QMSG and
 *     PM_REMOVE / PM_NOREMOVE; it can change the message, and TRUE swallows
 *     it (WinGetMsg then returns the next one)
 *   - HK_SENDMSG: called by WinSendMsg with an SMHSTRUCT before the window
 *     procedure; fInterTask for a send from another thread
 *   - HK_MSGFILTER through WinCallMsgFilter
 *   - HK_HELP through WM_HELP and WinDefWindowProc
 *   - WinInSendMsg in a window procedure sent a message by this thread and
 *     by another thread (while this one waits in WinWaitEventSem)
 *   - WinPostQueueMsg (a message for no window), WinBroadcastMsg
 *     (BMSG_SEND / BMSG_POST / BMSG_DESCENDANTS / BMSG_POSTQUEUE),
 *     WinWaitMsg, WinWaitEventSem, WinRequestMutexSem
 *
 * Each check is written to stdout:
 *
 *   pm_msg_hook_test: <name>: ok
 *   pm_msg_hook_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_msg_hook_test: done <passed>/<total>" when the program
 * ends.
 */
#define INCL_WIN
#define INCL_WINHOOKS
#define INCL_DOSPROCESS
#define INCL_DOSSEMAPHORES
#define INCL_DOSERRORS
#include <os2.h>
#include <string.h>

/* Not in the Open Watcom 32-bit <pmwin.h>; PM Reference / 1.x header. */
BOOL APIENTRY WinCallMsgFilter( HAB hab, PQMSG pqmsg, ULONG msgf );
#ifndef HLPM_FRAME
#define HLPM_FRAME  (-1)
#define HLPM_WINDOW (-2)
#define HLPM_MENU   (-3)
#endif

#define MSG_PLAIN     ( WM_USER + 1 )
#define MSG_SWALLOWED ( WM_USER + 2 )
#define MSG_NEXT      ( WM_USER + 3 )
#define MSG_CHANGED   ( WM_USER + 4 )
#define MSG_SENT      ( WM_USER + 10 )
#define MSG_OTHER     ( WM_USER + 11 )
#define MSG_SAME      ( WM_USER + 12 )
#define MSG_QUEUE     ( WM_USER + 20 )
#define MSG_BCAST     ( WM_USER + 30 )
#define MSG_BPOST     ( WM_USER + 31 )
#define MSG_BQUEUE    ( WM_USER + 32 )
#define MSG_WAIT      ( WM_USER + 40 )
#define MSG_FILTER    ( WM_USER + 50 )
#define MSGF_TEST     42
#define FIRST_TEST    WM_USER
#define LAST_TEST     ( WM_USER + 100 )

static HAB   hab;
static HMQ   hmq;
static HWND  frame, client;
static int   n_checks = 0;
static int   n_passed = 0;

/* What the hooks saw. */
static int   input_calls;
static ULONG input_msg, input_fs;
static int   send_calls;
static SMHSTRUCT send_smh;
static BOOL  send_inter;
static int   filter_calls;
static ULONG filter_msg, filter_msgf;
static int   help_calls;
static LONG  help_mode;
static ULONG help_topic, help_subtopic;
static RECTL help_rect;
static BOOL  help_rect_given;

/* What the window procedure saw. */
static int   bcast_calls;
static HWND  bcast_hwnd;
static BOOL  in_send_same = 99, in_send_other = 99;

/* The second thread. */
static HEV   thr_done;
static ULONG thr_result;

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
    char *p = append_str( line, "pm_msg_hook_test: " );
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

/* ---------- hooks ---------- */

static BOOL EXPENTRY InputHook( HAB h, PQMSG pq, ULONG fs )
{
    (void)h;
    input_calls++;
    input_msg = pq->msg;
    input_fs  = fs;
    if( pq->msg == MSG_SWALLOWED ) return TRUE;
    if( pq->msg == MSG_CHANGED ) pq->mp1 = MPFROMLONG( 99 );
    return FALSE;
}

static VOID EXPENTRY SendHook( HAB h, PSMHSTRUCT psmh, BOOL fInterTask )
{
    (void)h;
    send_calls++;
    send_smh   = *psmh;
    send_inter = fInterTask;
}

static BOOL EXPENTRY FilterHook( HAB h, PQMSG pq, ULONG msgf )
{
    (void)h;
    filter_calls++;
    filter_msg  = pq->msg;
    filter_msgf = msgf;
    return msgf == MSGF_TEST;
}

static BOOL EXPENTRY HelpHook( HAB h, ULONG usMode, ULONG idTopic, ULONG idSubTopic, PRECTL prc )
{
    (void)h;
    help_calls++;
    help_mode     = (SHORT)usMode;
    help_topic    = idTopic;
    help_subtopic = idSubTopic;
    help_rect_given = prc != NULL;
    if( prc != NULL ) help_rect = *prc;
    return TRUE;
}

/* ---------- the window procedure ---------- */

static MRESULT EXPENTRY ClientWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case MSG_SENT:
        return MRFROMLONG( LONGFROMMP( mp1 ) + LONGFROMMP( mp2 ) );
    case MSG_SAME:
        in_send_same = WinInSendMsg( hab );
        return 0;
    case MSG_OTHER:
        in_send_other = WinInSendMsg( hab );
        return MRFROMLONG( 7 );
    case MSG_BCAST:
        bcast_calls++;
        bcast_hwnd = hwnd;
        return 0;
    }
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

/* ---------- the second thread ---------- */

static VOID APIENTRY second_thread( ULONG arg )
{
    HAB hab2 = WinInitialize( 0 );
    HMQ hmq2 = WinCreateMsgQueue( hab2, 0 );

    (void)arg;
    thr_result = LONGFROMMR( WinSendMsg( client, MSG_OTHER, 0, 0 ) );
    WinDestroyMsgQueue( hmq2 );
    WinTerminate( hab2 );
    DosPostEventSem( thr_done );
}

int main( void )
{
    ULONG   flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_SIZEBORDER;
    QMSG    qmsg;
    TID     tid;
    HMTX    mtx;
    HEV     sem;
    char    line[64], *p;

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "MsgHookTest", ClientWndProc, 0, 0 );
    frame = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &flags, "MsgHookTest",
                                "PM Message Hook Test", 0, NULLHANDLE, 0, &client );

    /* ---- the input hook ---- */
    check( "WinSetHook(HK_INPUT)", WinSetHook( hab, HMQ_CURRENT, HK_INPUT, (PFN)InputHook, NULLHANDLE ), TRUE );
    WinPostMsg( client, MSG_PLAIN, MPFROMLONG( 5 ), 0 );
    check( "WinGetMsg with an input hook", WinGetMsg( hab, &qmsg, NULLHANDLE, FIRST_TEST, LAST_TEST ), TRUE );
    check( "-> the message", qmsg.msg, MSG_PLAIN );
    check( "-> the hook was called", input_calls, 1 );
    check( "-> with the message", input_msg, MSG_PLAIN );
    check( "-> and PM_REMOVE", input_fs, PM_REMOVE );

    WinPostMsg( client, MSG_SWALLOWED, 0, 0 );
    WinPostMsg( client, MSG_NEXT, 0, 0 );
    input_calls = 0;
    check( "WinGetMsg after a swallowed message", WinGetMsg( hab, &qmsg, NULLHANDLE, FIRST_TEST, LAST_TEST ), TRUE );
    check( "-> the next message", qmsg.msg, MSG_NEXT );
    check( "-> the hook saw both", input_calls, 2 );

    WinPostMsg( client, MSG_CHANGED, MPFROMLONG( 1 ), 0 );
    WinGetMsg( hab, &qmsg, NULLHANDLE, FIRST_TEST, LAST_TEST );
    check( "a hook changes the message", LONGFROMMP( qmsg.mp1 ), 99 );

    WinPostMsg( client, MSG_PLAIN, 0, 0 );
    check( "WinPeekMsg(PM_NOREMOVE)", WinPeekMsg( hab, &qmsg, client, MSG_PLAIN, MSG_PLAIN, PM_NOREMOVE ), TRUE );
    check( "-> the hook got PM_NOREMOVE", input_fs, PM_NOREMOVE );
    check( "WinPeekMsg(PM_REMOVE)", WinPeekMsg( hab, &qmsg, client, MSG_PLAIN, MSG_PLAIN, PM_REMOVE ), TRUE );
    check( "-> the hook got PM_REMOVE", input_fs, PM_REMOVE );

    check( "WinReleaseHook(HK_INPUT)", WinReleaseHook( hab, HMQ_CURRENT, HK_INPUT, (PFN)InputHook, NULLHANDLE ), TRUE );
    check( "WinReleaseHook(HK_INPUT) again", WinReleaseHook( hab, HMQ_CURRENT, HK_INPUT, (PFN)InputHook, NULLHANDLE ), FALSE );
    input_calls = 0;
    WinPostMsg( client, MSG_PLAIN, 0, 0 );
    WinGetMsg( hab, &qmsg, NULLHANDLE, FIRST_TEST, LAST_TEST );
    check( "a released hook is not called", input_calls, 0 );

    /* ---- the send hook ---- */
    check( "WinSetHook(HK_SENDMSG)", WinSetHook( hab, HMQ_CURRENT, HK_SENDMSG, (PFN)SendHook, NULLHANDLE ), TRUE );
    check( "WinSendMsg with a send hook", LONGFROMMR( WinSendMsg( client, MSG_SENT, MPFROMLONG( 3 ), MPFROMLONG( 4 ) ) ), 7 );
    check( "-> the hook was called", send_calls, 1 );
    check( "-> SMHSTRUCT.msg", send_smh.msg, MSG_SENT );
    check( "-> SMHSTRUCT.hwnd", send_smh.hwnd, client );
    check( "-> SMHSTRUCT.mp1", LONGFROMMP( send_smh.mp1 ), 3 );
    check( "-> SMHSTRUCT.mp2", LONGFROMMP( send_smh.mp2 ), 4 );
    check( "-> SMHSTRUCT.model", send_smh.model, PM_MODEL_2X );
    check( "-> not inter-task", send_inter, FALSE );

    /* ---- WinInSendMsg ---- */
    WinSendMsg( client, MSG_SAME, 0, 0 );
    check( "WinInSendMsg in a message this thread sent", in_send_same, FALSE );
    DosCreateEventSem( NULL, &thr_done, 0, FALSE );
    DosCreateThread( &tid, second_thread, 0, 0, 0x8000 );
    check( "WinWaitEventSem(second thread done)", WinWaitEventSem( thr_done, 5000 ), NO_ERROR );
    check( "WinInSendMsg in a message another thread sent", in_send_other, TRUE );
    check( "-> its WinSendMsg result", thr_result, 7 );
    check( "-> the send hook saw an inter-task send", send_inter, TRUE );
    check( "-> of that message", send_smh.msg, MSG_OTHER );
    check( "WinReleaseHook(HK_SENDMSG)", WinReleaseHook( hab, HMQ_CURRENT, HK_SENDMSG, (PFN)SendHook, NULLHANDLE ), TRUE );
    send_calls = 0;
    WinSendMsg( client, MSG_SENT, 0, 0 );
    check( "-> not called any more", send_calls, 0 );

    /* ---- WinPostQueueMsg ---- */
    check( "WinPostQueueMsg", WinPostQueueMsg( hmq, MSG_QUEUE, MPFROMLONG( 6 ), 0 ), TRUE );
    WinGetMsg( hab, &qmsg, NULLHANDLE, FIRST_TEST, LAST_TEST );
    check( "-> the message", qmsg.msg, MSG_QUEUE );
    check( "-> for no window", qmsg.hwnd, NULLHANDLE );
    check( "-> mp1", LONGFROMMP( qmsg.mp1 ), 6 );
    check( "WinPostQueueMsg(bad hmq)", WinPostQueueMsg( (HMQ)0xDEAD, MSG_QUEUE, 0, 0 ), FALSE );

    /* ---- WinBroadcastMsg ---- */
    check( "WinBroadcastMsg(BMSG_SEND | BMSG_DESCENDANTS)",
           WinBroadcastMsg( frame, MSG_BCAST, 0, 0, BMSG_SEND | BMSG_DESCENDANTS ), TRUE );
    check( "-> the client got it once", bcast_calls, 1 );
    check( "-> as itself", bcast_hwnd, client );
    check( "WinBroadcastMsg(BMSG_POST)", WinBroadcastMsg( frame, MSG_BPOST, 0, 0, BMSG_POST ), TRUE );
    check( "-> posted to the client", WinPeekMsg( hab, &qmsg, client, MSG_BPOST, MSG_BPOST, PM_REMOVE ), TRUE );
    check( "WinBroadcastMsg(BMSG_POSTQUEUE)", WinBroadcastMsg( HWND_DESKTOP, MSG_BQUEUE, 0, 0, BMSG_POSTQUEUE ), TRUE );
    check( "-> posted to the queue", WinPeekMsg( hab, &qmsg, NULLHANDLE, MSG_BQUEUE, MSG_BQUEUE, PM_REMOVE ), TRUE );
    check( "-> for no window", qmsg.hwnd, NULLHANDLE );

    /* ---- WinWaitMsg ---- */
    WinPostQueueMsg( hmq, MSG_WAIT, 0, 0 );
    check( "WinWaitMsg", WinWaitMsg( hab, MSG_WAIT, MSG_WAIT ), TRUE );
    check( "-> leaves the message", WinPeekMsg( hab, &qmsg, NULLHANDLE, MSG_WAIT, MSG_WAIT, PM_REMOVE ), TRUE );

    /* ---- semaphores ---- */
    DosCreateEventSem( NULL, &sem, 0, FALSE );
    check( "WinWaitEventSem(reset, 10 ms)", WinWaitEventSem( sem, 10 ), ERROR_TIMEOUT );
    DosPostEventSem( sem );
    check( "WinWaitEventSem(posted)", WinWaitEventSem( sem, 10 ), NO_ERROR );
    DosCreateMutexSem( NULL, &mtx, 0, FALSE );
    check( "WinRequestMutexSem", WinRequestMutexSem( mtx, SEM_INDEFINITE_WAIT ), NO_ERROR );
    DosReleaseMutexSem( mtx );

    /* ---- the message-filter hook ---- */
    qmsg.hwnd = client;
    qmsg.msg  = MSG_FILTER;
    check( "WinCallMsgFilter without hooks", WinCallMsgFilter( hab, &qmsg, MSGF_TEST ), FALSE );
    check( "WinSetHook(HK_MSGFILTER)", WinSetHook( hab, HMQ_CURRENT, HK_MSGFILTER, (PFN)FilterHook, NULLHANDLE ), TRUE );
    check( "WinCallMsgFilter, the hook handles it", WinCallMsgFilter( hab, &qmsg, MSGF_TEST ), TRUE );
    check( "-> the hook got the message", filter_msg, MSG_FILTER );
    check( "-> and the filter code", filter_msgf, MSGF_TEST );
    check( "WinCallMsgFilter, the hook leaves it", WinCallMsgFilter( hab, &qmsg, MSGF_TEST + 1 ), FALSE );
    check( "-> called twice", filter_calls, 2 );
    WinReleaseHook( hab, HMQ_CURRENT, HK_MSGFILTER, (PFN)FilterHook, NULLHANDLE );

    /* ---- the help hook ---- */
    check( "WinSetHook(HK_HELP)", WinSetHook( hab, HMQ_CURRENT, HK_HELP, (PFN)HelpHook, NULLHANDLE ), TRUE );
    WinSendMsg( client, WM_HELP, MPFROMSHORT( 5 ), MPFROM2SHORT( CMDSRC_PUSHBUTTON, FALSE ) );
    check( "WM_HELP calls the help hook", help_calls, 1 );
    check( "-> in a frame", help_mode, HLPM_FRAME );
    check( "-> with a position", help_rect_given, TRUE );
    check( "-> that has a width", help_rect.xRight > help_rect.xLeft, TRUE );
    WinSendMsg( client, WM_HELP, MPFROMSHORT( 0x123 ), MPFROM2SHORT( CMDSRC_MENU, FALSE ) );
    check( "WM_HELP from a menu", help_mode, HLPM_MENU );
    check( "-> the item", help_subtopic, 0x123 );
    WinReleaseHook( hab, HMQ_CURRENT, HK_HELP, (PFN)HelpHook, NULLHANDLE );

    /* ---- bad arguments ---- */
    check( "WinSetHook(bad hmq)", WinSetHook( hab, (HMQ)0xDEAD, HK_INPUT, (PFN)InputHook, NULLHANDLE ), FALSE );
    check( "WinSetHook(unknown hook type)", WinSetHook( hab, HMQ_CURRENT, 14, (PFN)InputHook, NULLHANDLE ), FALSE );

    WinDestroyWindow( frame );
    WinDestroyMsgQueue( hmq );

    p = append_str( line, "pm_msg_hook_test: done " );
    p = append_long( p, n_passed );
    p = append_str( p, "/" );
    p = append_long( p, n_checks );
    append_str( p, "\r\n" );
    write_stdout( line );

    WinTerminate( hab );
    return 0;
}
