/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_dlg_signal_test.c - signal exceptions reach thread 1 while it waits
 * in a modal dialog loop (ADR-0093).
 *
 * Thread 1 registers an exception handler that continues XCPT_SIGNAL,
 * takes the signal focus and runs WinDlgBox. A second thread sends
 * XCPT_SIGNAL_INTR to the process, waits for the handler, posts a message
 * to the dialog, sends XCPT_SIGNAL_BREAK, waits again, then posts the
 * message that dismisses the dialog with 42. Each check is written as
 *
 *   pm_dlg_signal_test: <name>: ok
 *   pm_dlg_signal_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_dlg_signal_test: done <passed>/<total>".
 */
#define INCL_DOS
#define INCL_DOSERRORS
#define INCL_WIN
#include <os2.h>
#include <stdio.h>
#include "pm_dlg_signal_test.h"

#define WM_STEP     (WM_USER + 1)   /* a message between the signals */
#define WM_FINISH   (WM_USER + 2)   /* dismisses the dialog */
#define WAIT_MS     5000

static volatile HWND  dlg_hwnd;
static volatile int   in_dialog;            /* between WM_INITDLG and the dismissal */
static volatile int   n_signals;
static volatile ULONG signals[2];
static volatile int   signals_in_dialog;
static volatile int   step_after_first;     /* WM_STEP came after the first signal */
static int n_checks, n_passed;

static void check( const char *what, LONG got, LONG want )
{
    n_checks++;
    if( got == want ) {
        n_passed++;
        printf( "pm_dlg_signal_test: %s: ok\n", what );
    } else {
        printf( "pm_dlg_signal_test: %s: FAIL got %ld want %ld\n", what, got, want );
    }
}

static ULONG APIENTRY handler( PEXCEPTIONREPORTRECORD rep, PEXCEPTIONREGISTRATIONRECORD reg,
                               PCONTEXTRECORD ctx, PVOID unused )
{
    (void)reg; (void)ctx; (void)unused;
    if( rep->ExceptionNum == XCPT_SIGNAL && n_signals < 2 ) {
        signals[n_signals] = rep->ExceptionInfo[0];
        if( in_dialog ) signals_in_dialog++;
        n_signals++;
        return XCPT_CONTINUE_EXECUTION;
    }
    return XCPT_CONTINUE_SEARCH;
}

static int wait_signals( int n )
{
    int ms;
    for( ms = 0; ms < WAIT_MS && n_signals < n; ms += 20 ) DosSleep( 20 );
    return n_signals >= n;
}

static VOID APIENTRY sender( ULONG arg )
{
    PPIB ppib;
    PTIB ptib;
    int ms;
    (void)arg;
    for( ms = 0; ms < WAIT_MS && dlg_hwnd == NULLHANDLE; ms += 20 ) DosSleep( 20 );
    DosSleep( 200 );
    DosGetInfoBlocks( &ptib, &ppib );
    DosSendSignalException( ppib->pib_ulpid, XCPT_SIGNAL_INTR );
    wait_signals( 1 );
    WinPostMsg( dlg_hwnd, WM_STEP, 0, 0 );
    DosSleep( 200 );
    DosSendSignalException( ppib->pib_ulpid, XCPT_SIGNAL_BREAK );
    wait_signals( 2 );
    WinPostMsg( dlg_hwnd, WM_FINISH, 0, 0 );
}

static MRESULT EXPENTRY dlg_proc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case WM_INITDLG:
        in_dialog = 1;
        dlg_hwnd = hwnd;
        return 0;
    case WM_STEP:
        step_after_first = n_signals == 1;
        return 0;
    case WM_FINISH:
        in_dialog = 0;
        WinDismissDlg( hwnd, 42 );
        return 0;
    }
    return WinDefDlgProc( hwnd, msg, mp1, mp2 );
}

int main( void )
{
    HAB hab = WinInitialize( 0 );
    HMQ hmq = WinCreateMsgQueue( hab, 0 );
    EXCEPTIONREGISTRATIONRECORD rec;
    ULONG times;
    TID tid;
    ULONG result;

    rec.prev_structure = NULL;
    rec.ExceptionHandler = handler;
    DosSetExceptionHandler( &rec );
    DosSetSignalExceptionFocus( SIG_SETFOCUS, &times );
    DosCreateThread( &tid, sender, 0, 0, 65536 );

    result = WinDlgBox( HWND_DESKTOP, HWND_DESKTOP, dlg_proc, NULLHANDLE, ID_DLG, NULL );

    check( "WinDlgBox returns the dismissal's result", (LONG)result, 42 );
    check( "two signals reached the handler", n_signals, 2 );
    check( "the first was XCPT_SIGNAL_INTR", (LONG)signals[0], XCPT_SIGNAL_INTR );
    check( "the second was XCPT_SIGNAL_BREAK", (LONG)signals[1], XCPT_SIGNAL_BREAK );
    check( "both while the dialog was up", signals_in_dialog, 2 );
    check( "the dialog took messages after the first", step_after_first, 1 );
    check( "the sender ended", DosWaitThread( &tid, DCWW_WAIT ), NO_ERROR );

    DosUnsetExceptionHandler( &rec );
    printf( "pm_dlg_signal_test: done %d/%d\n", n_passed, n_checks );
    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return n_passed == n_checks ? 0 : 1;
}
