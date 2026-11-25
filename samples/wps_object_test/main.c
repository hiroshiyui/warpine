/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * main.c - wps_object_test: WPObject and M_WPObject from the SDK's libwps
 * (ADR-0129), through a class of the program's own (note.idl):
 *   - wpclsNew: no object of WPObject itself; a Note titled by its class,
 *     styled by its class, set up from a setup string (its own TEXT key
 *     beside WPObject's TITLE / OBJECTID / DEFAULTVIEW / style keys, with
 *     "^;" for a semicolon), locked, initialized once (wpInitData) and its
 *     class initialized (wpclsInitData);
 *   - wpScanSetupString's buffer sizes, wpSetTitle's limit, object IDs;
 *   - wpSaveState / wpRestoreState through wpSaveString and the like,
 *     wpRestoreData's size query;
 *   - wpAllocMem / wpFreeMem;
 *   - wpDelete refused for OBJSTYLE_NODELETE, then done (wpUnInitData);
 *   - an object made by somNew: initialized the same way, the class's
 *     default view.
 * Each check is written as "wps_object_test: <name>: ok" or "... FAIL",
 * then "wps_object_test: done <passed>/<total>".
 */
#define INCL_WINWORKPLACE
#include <os2.h>
#include <stdio.h>
#include <string.h>
#include "note.h"

extern int note_inits, note_uninits, note_class_inits;

static int passed, total;

static void check( const char *name, int ok )
{
    total++;
    if( ok ) passed++;
    printf( "wps_object_test: %s: %s\n", name, ok ? "ok" : "FAIL" );
}

int main( void )
{
    Note  *note, *plain;
    char   buf[300];
    ULONG  cb, value, rc;
    PBYTE  mem;

    check( "wpclsNew: no object of WPObject itself",
           _wpclsNew( _WPObject, "x", "", NULL, FALSE ) == NULL );

    note = (Note *)_wpclsNew( _Note, "", "TEXT=hello^; world;NODELETE=YES;OBJECTID=<WP_NOTE1>;DEFAULTVIEW=SETTINGS",
                              NULL, TRUE );
    check( "wpclsNew: a Note", note != NULL && _somIsA( note, _WPObject ) );
    if( !note ) { printf( "wps_object_test: done %d/%d\n", passed, total ); return( 1 ); }
    check( "wpclsInitData: the class initialized once", note_class_inits == 1 );
    check( "wpInitData: once, from somNew", note_inits == 1 );
    check( "wpclsQueryTitle: the title of an untitled object", strcmp( _wpQueryTitle( note ), "Note pad" ) == 0 );
    check( "wpSetup: the class's TEXT key, ^; a semicolon", strcmp( _queryText( note ), "hello; world" ) == 0 );
    check( "wpSetup: NODELETE beside the class's style",
           _wpQueryStyle( note ) == ( OBJSTYLE_NODELETE | OBJSTYLE_NOPRINT ) );
    check( "wpSetup: OBJECTID", _wpQueryObjectID( note ) && strcmp( _wpQueryObjectID( note ), "<WP_NOTE1>" ) == 0 );
    check( "wpSetup: DEFAULTVIEW=SETTINGS", _wpQueryDefaultView( note ) == OPEN_SETTINGS );
    check( "wpclsNew: locked, initialized", _wpIsLocked( note ) && _wpIsObjectInitialized( note ) );

    /* wpScanSetupString: the size alone, a buffer too small, a key that
     * is not there. */
    cb = 0;
    check( "wpScanSetupString: the size alone",
           _wpScanSetupString( note, "TITLE=Abc;X=1", "title", NULL, &cb ) && cb == 4 );
    cb = 2;
    check( "wpScanSetupString: a buffer too small",
           !_wpScanSetupString( note, "TITLE=Abc", "TITLE", buf, &cb ) && cb == 4 );
    cb = sizeof( buf );
    check( "wpScanSetupString: no such key", !_wpScanSetupString( note, "TITLE=Abc", "TEXT", buf, &cb ) );

    check( "wpSetTitle", _wpSetTitle( note, "Shopping" ) && strcmp( _wpQueryTitle( note ), "Shopping" ) == 0 );
    memset( buf, 'a', 256 );
    buf[256] = '\0';
    check( "wpSetTitle: 256 characters are too many", !_wpSetTitle( note, buf ) && strcmp( _wpQueryTitle( note ), "Shopping" ) == 0 );
    check( "wpSetObjectID: <NAME> only", !_wpSetObjectID( note, "NOTE" ) && _wpSetObjectID( note, "<WP_NOTE2>" ) );

    /* Save and restore: the class's own state through wpSaveString. */
    check( "wpSaveImmediate: wpSaveState", _wpSaveImmediate( note ) );
    _wpSetup( note, "TEXT=changed" );
    check( "wpRestoreState: the saved text back",
           _wpRestoreState( note, 0 ) && strcmp( _queryText( note ), "hello; world" ) == 0 );
    check( "wpSaveLong / wpRestoreLong",
           _wpSaveLong( note, "Main", 7, 0xC0FFEE ) && _wpRestoreLong( note, "Main", 7, &value ) && value == 0xC0FFEE );
    check( "wpRestoreLong: another class's key is not there", !_wpRestoreLong( note, "Other", 7, &value ) );
    cb = 0;
    check( "wpSaveData / wpRestoreData: the size alone, then the data",
           _wpSaveData( note, "Main", 8, (PBYTE)"\1\2\3", 3 ) && _wpRestoreData( note, "Main", 8, NULL, &cb ) && cb == 3
           && _wpRestoreData( note, "Main", 8, (PBYTE)buf, &cb ) && memcmp( buf, "\1\2\3", 3 ) == 0 );

    mem = _wpAllocMem( note, 64, &rc );
    check( "wpAllocMem / wpFreeMem", mem && rc == NO_ERROR && _wpFreeMem( note, mem ) && !_wpFreeMem( note, mem ) );
    check( "wpSetError / wpQueryError", _wpSetError( note, 1234 ) && _wpQueryError( note ) == 1234 );
    check( "wpUnlockObject", _wpUnlockObject( note ) && !_wpIsLocked( note ) && !_wpUnlockObject( note ) );

    /* wpDelete: refused while OBJSTYLE_NODELETE is set. */
    check( "wpDelete: NO_DELETE for OBJSTYLE_NODELETE", _wpDelete( note, 0 ) == NO_DELETE && note_uninits == 0 );
    _wpSetStyle( note, _wpQueryStyle( note ) & ~OBJSTYLE_NODELETE );
    check( "wpDelete: OK_DELETE, the object uninitialized", _wpDelete( note, 0 ) == OK_DELETE && note_uninits == 1 );

    /* An object made by somNew: initialized all the same. */
    plain = NoteNew();
    check( "somNew: wpInitData ran", plain != NULL && note_inits == 2 && note_class_inits == 1 );
    check( "wpQueryDefaultView: the class's (OPEN_SETTINGS)", _wpQueryDefaultView( plain ) == OPEN_SETTINGS );
    check( "wpQueryStyle: the class's style", _wpQueryStyle( plain ) == OBJSTYLE_NOPRINT );
    check( "wpQueryTitle: empty before a title is set", strcmp( _wpQueryTitle( plain ), "" ) == 0 );
    check( "wpFree: wpUnInitData ran", _wpFree( plain ) && note_uninits == 2 );

    printf( "wps_object_test: done %d/%d\n", passed, total );
    return( passed == total ? 0 : 1 );
}
