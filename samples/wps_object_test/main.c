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
 *     default view;
 *   - WPAbstract (Card): stored at once with a handle WinQueryObject finds
 *     by its object ID, which no second object may take; a change saved
 *     by itself; made dormant (somFree) and awakened by wpclsQueryObject
 *     with its title, text and ID; wpFree removes it;
 *   - WinCreateObject (ADR-0130): a Card made when it is first asked for,
 *     its title and setup applied; an ID taken (CO_FAILIFEXISTS,
 *     CO_UPDATEIFEXISTS, CO_REPLACEIFEXISTS); WinSetObjectData, applied
 *     also to an awake object; WinDestroyObject;
 *   - WPFileSystem / WPDataFile: the object of a file the program wrote
 *     (wpclsQueryObjectFromPath), its names, size, dates and types (.TYPE),
 *     its title renaming the file (.LONGNAME for a title no name can be),
 *     kept in its EAs and awakened again after somFree, refused deletion,
 *     then wpFree deleting the file; no object for a directory without
 *     WPFolder; WPTransient, never stored.
 * "save" stores a Card and has another created by WinCreateObject without
 * making it, and ends; a later "load" (another process) finds both by
 * their IDs, makes or awakens them, checks them and frees them.
 * Each check is written as "wps_object_test: <name>: ok" or "... FAIL",
 * then "wps_object_test: done <passed>/<total>".
 */
#define INCL_WINWORKPLACE
#include <os2.h>
#include <stdio.h>
#include <string.h>
#include <wptrans.h>
#include <wpdataf.h>
#include "note.h"

extern int note_inits, note_uninits, note_class_inits;

#define CARD_ID    "<WP_CARD_T>"
#define STORED_ID  "<WP_CARD_P>"
#define CREATED_ID "<WP_CARD_C>"
#define QUEUED_ID  "<WP_CARD_Q>"

static int passed, total;

static void check( const char *name, int ok )
{
    total++;
    if( ok ) passed++;
    printf( "wps_object_test: %s: %s\n", name, ok ? "ok" : "FAIL" );
}

/* WPAbstract within this process. */
static void abstract_checks( void )
{
    Card    *card, *again;
    HOBJECT  h;

    card = (Card *)_wpclsNew( _Card, "Recipe", "OBJECTID=" CARD_ID ";TEXT=flour", NULL, FALSE );
    h = card ? _wpQueryHandle( card ) : NULLHANDLE;
    check( "WPAbstract: stored at once, with a handle", card && h != NULLHANDLE );
    if( !card ) return;
    check( "WinQueryObject: the handle by the object ID", WinQueryObject( CARD_ID ) == h );
    check( "wpQueryHandle: none for an object without storage", _wpQueryHandle( NoteNew() ) == NULLHANDLE );
    check( "WPAbstract: a second object may not take the ID",
           _wpclsNew( _Card, "Copy", "OBJECTID=" CARD_ID, NULL, FALSE ) == NULL );
    check( "wpclsQueryObject: the awake object", _wpclsQueryObject( _WPObject, h ) == card );
    /* A change is saved by itself; a dormant object comes back from the
     * user profile. */
    _wpSetTitle( card, "Pancakes" );
    _somFree( card );
    again = (Card *)_wpclsQueryObject( _WPObject, h );
    check( "wpclsQueryObject: awakened from the user profile",
           again && _somIsA( again, _Card ) && _wpQueryHandle( again ) == h && _wpIsObjectInitialized( again ) );
    if( !again ) return;
    check( "wpRestoreState: the title changed before", strcmp( _wpQueryTitle( again ), "Pancakes" ) == 0 );
    check( "wpRestoreState: the class's own text", strcmp( _cardText( again ), "flour" ) == 0 );
    check( "wpRestoreState: the object ID",
           _wpQueryObjectID( again ) && strcmp( _wpQueryObjectID( again ), CARD_ID ) == 0 );
    _wpFree( again );
    check( "wpFree: no longer stored",
           WinQueryObject( CARD_ID ) == NULLHANDLE && _wpclsQueryObject( _WPObject, h ) == NULL );
}

/* WinCreateObject, WinSetObjectData and WinDestroyObject. */
static void create_checks( void )
{
    HOBJECT h, h2;
    Card   *card;

    h = WinCreateObject( "Card", "Lunch", "TEXT=soup;OBJECTID=" CREATED_ID, "<WP_DESKTOP>", CO_FAILIFEXISTS );
    check( "WinCreateObject: a handle, found by the ID", h != NULLHANDLE && WinQueryObject( CREATED_ID ) == h );
    check( "WinCreateObject: refused for a taken ID (fail if exists)",
           WinCreateObject( "Card", "Other", "OBJECTID=" CREATED_ID, "<WP_DESKTOP>", CO_FAILIFEXISTS ) == NULLHANDLE );
    check( "WinCreateObject: CO_UPDATEIFEXISTS gives the object",
           WinCreateObject( "Card", "", "TEXT=bread;OBJECTID=" CREATED_ID, "<WP_DESKTOP>", CO_UPDATEIFEXISTS ) == h );
    check( "WinSetObjectData: before the object is made", WinSetObjectData( h, "TEXT=stew" ) );
    card = (Card *)_wpclsQueryObject( _WPObject, h );
    check( "wpclsQueryObject: made from what was queued, in order",
           card && _somIsA( card, _Card ) && strcmp( _wpQueryTitle( card ), "Lunch" ) == 0
           && strcmp( _cardText( card ), "stew" ) == 0 && _wpQueryHandle( card ) == h );
    if( !card ) return;
    check( "WinSetObjectData: an awake object, applied when asked for again",
           WinSetObjectData( h, "TEXT=pie;TITLE=Dinner" ) && _wpclsQueryObject( _WPObject, h ) == card
           && strcmp( _cardText( card ), "pie" ) == 0 && strcmp( _wpQueryTitle( card ), "Dinner" ) == 0 );
    h2 = WinCreateObject( "Card", "New", "OBJECTID=" CREATED_ID, "<WP_DESKTOP>", CO_REPLACEIFEXISTS );
    check( "WinCreateObject: CO_REPLACEIFEXISTS, a new object for the ID",
           h2 != NULLHANDLE && h2 != h && WinQueryObject( CREATED_ID ) == h2 && !WinSetObjectData( h, "TEXT=x" ) );
    _somFree( card );
    check( "WinDestroyObject", WinDestroyObject( h2 ) && WinQueryObject( CREATED_ID ) == NULLHANDLE
           && !WinDestroyObject( h2 ) && _wpclsQueryObject( _WPObject, h2 ) == NULL );
    check( "WinCreateObject: a class no program has made is created all the same, and not made",
           ( h = WinCreateObject( "NoSuchClass", "x", "", "<WP_DESKTOP>", 0 ) ) != NULLHANDLE
           && _wpclsQueryObject( _WPObject, h ) == NULL && WinDestroyObject( h ) );
}

/* A file of `text`, created. */
static BOOL make_file( const char *name, const char *text )
{
    HFILE  hf;
    ULONG  action, written;
    if( DosOpen( name, &hf, &action, 0, FILE_NORMAL, OPEN_ACTION_CREATE_IF_NEW | OPEN_ACTION_REPLACE_IF_EXISTS,
                 OPEN_ACCESS_WRITEONLY | OPEN_SHARE_DENYNONE, NULL ) != NO_ERROR )
        return( FALSE );
    DosWrite( hf, text, strlen( text ), &written );
    DosClose( hf );
    return( TRUE );
}

static BOOL exists( const char *name )
{
    FILESTATUS3 st;
    return( DosQueryPathInfo( name, FIL_STANDARD, &st, sizeof( st ) ) == NO_ERROR );
}

/* WPFileSystem, WPDataFile and WPTransient. */
static void file_checks( void )
{
    char          full[CCHMAXPATH], buf[CCHMAXPATH];
    ULONG         cb;
    FDATE         date;
    WPFileSystem *file, *again;
    WPObject     *temp;

    if( !make_file( "WPSFILE.TXT", "hello" ) || DosQueryPathInfo( "WPSFILE.TXT", FIL_QUERYFULLNAME, full, sizeof( full ) ) ) {
        check( "files: a file written", FALSE );
        return;
    }
    file = (WPFileSystem *)_wpclsQueryObjectFromPath( _WPFileSystem, full );
    check( "wpclsQueryObjectFromPath: a WPDataFile, locked, titled by its name",
           file && _somIsA( file, _WPDataFile ) && _wpIsLocked( file ) && strcmp( _wpQueryTitle( file ), "WPSFILE.TXT" ) == 0 );
    if( !file ) return;
    check( "wpclsQueryObjectFromPath: one object per file, any case",
           _wpclsQueryObjectFromPath( _WPFileSystem, "wpsfile.txt" ) == file );
    cb = sizeof( buf );
    check( "wpQueryRealName: qualified and not",
           _wpQueryRealName( file, buf, &cb, TRUE ) && strcmp( buf, full ) == 0
           && _wpQueryFilename( file, buf, FALSE ) && strcmp( buf, "WPSFILE.TXT" ) == 0 );
    cb = 2;
    check( "wpQueryRealName: a buffer too small gives the size", !_wpQueryRealName( file, buf, &cb, FALSE ) && cb == 12 );
    check( "wpQueryFileSize / wpQueryLastWrite / wpQueryEASupport",
           _wpQueryFileSize( file ) == 5 && _wpQueryLastWrite( file, &date, NULL ) == NO_ERROR && date.year > 0
           && _wpQueryEASupport( file ) == NO_ERROR );
    check( "wpSetType / wpQueryType: .TYPE, line feeds between types",
           _wpSetType( file, "Plain Text\nC Code", NULL ) && strcmp( _wpQueryType( file ), "Plain Text\nC Code" ) == 0 );
    /* A title is the file's name: setting it renames the file. */
    check( "wpSetTitle: the file renamed", _wpSetTitle( file, "Notes.txt" ) && exists( "Notes.txt" ) && !exists( "WPSFILE.TXT" )
           && _wpQueryFilename( file, buf, FALSE ) && strcmp( buf, "Notes.txt" ) == 0 );
    check( "wpSetTitle: a title no file name can be, in .LONGNAME",
           _wpSetTitle( file, "Plans: 2026/27" ) && exists( "Plans_ 2026_27" ) && strcmp( _wpQueryTitle( file ), "Plans: 2026/27" ) == 0 );
    _wpSetStyle( file, _wpQueryStyle( file ) | OBJSTYLE_NODELETE );
    /* Dormant, then awakened from its EAs. */
    cb = sizeof( full );
    _wpQueryRealName( file, full, &cb, TRUE );
    _somFree( file );
    again = (WPFileSystem *)_wpclsQueryObjectFromPath( _WPFileSystem, full );
    check( "wpclsQueryObjectFromPath: awakened from the file's EAs",
           again && again != NULL && strcmp( _wpQueryTitle( again ), "Plans: 2026/27" ) == 0
           && strcmp( _wpQueryType( again ), "Plain Text\nC Code" ) == 0 && ( _wpQueryStyle( again ) & OBJSTYLE_NODELETE ) );
    if( !again ) return;
    check( "wpDelete: refused for OBJSTYLE_NODELETE, the file stays", _wpDelete( again, 0 ) == NO_DELETE && exists( full ) );
    _wpSetStyle( again, _wpQueryStyle( again ) & ~OBJSTYLE_NODELETE );
    check( "wpDelete: the file deleted", _wpDelete( again, 0 ) == OK_DELETE && !exists( full ) );
    check( "wpclsQueryObjectFromPath: none for a file that is not there",
           _wpclsQueryObjectFromPath( _WPFileSystem, "NOFILE.TXT" ) == NULL );
    DosCreateDir( "WPSDIR", NULL );
    check( "wpclsQueryObjectFromPath: no object for a directory without WPFolder",
           _wpclsQueryObjectFromPath( _WPFileSystem, "WPSDIR" ) == NULL );
    DosDeleteDir( "WPSDIR" );
    temp = _wpclsNew( _WPTransient, "Temporary", "", NULL, FALSE );
    check( "WPTransient: made, no handle, nothing stored",
           temp && _wpQueryHandle( temp ) == NULLHANDLE && _wpSaveImmediate( temp ) && _wpFree( temp ) );
}

/* "save": a Card stored for a later process. */
static int save_mode( void )
{
    Card *card = (Card *)_wpclsNew( _Card, "Kept", "OBJECTID=" STORED_ID ";TEXT=from another process", NULL, FALSE );
    check( "save: stored", card && WinQueryObject( STORED_ID ) == _wpQueryHandle( card ) );
    check( "save: one queued by WinCreateObject",
           WinCreateObject( "Card", "Queued", "OBJECTID=" QUEUED_ID ";TEXT=made later", "<WP_DESKTOP>", 0 ) != NULLHANDLE );
    printf( "wps_object_test: done %d/%d\n", passed, total );
    return( passed == total ? 0 : 1 );
}

/* "load": the Card another process stored. */
static int load_mode( void )
{
    HOBJECT h;
    Card   *card;
    h = WinQueryObject( STORED_ID );
    check( "load: WinQueryObject finds the stored object", h != NULLHANDLE );
    card = (Card *)_wpclsQueryObject( _WPObject, h );
    check( "load: awakened, its title and text", card && strcmp( _wpQueryTitle( card ), "Kept" ) == 0
           && strcmp( _cardText( card ), "from another process" ) == 0 );
    if( card ) _wpFree( card );
    check( "load: wpFree removed it", WinQueryObject( STORED_ID ) == NULLHANDLE );
    card = (Card *)_wpclsQueryObject( _WPObject, WinQueryObject( QUEUED_ID ) );
    check( "load: WinCreateObject's object made here", card && strcmp( _wpQueryTitle( card ), "Queued" ) == 0
           && strcmp( _cardText( card ), "made later" ) == 0 );
    if( card ) _wpFree( card );
    printf( "wps_object_test: done %d/%d\n", passed, total );
    return( passed == total ? 0 : 1 );
}

int main( int argc, char **argv )
{
    Note  *note, *plain;
    char   buf[300];
    ULONG  cb, value, rc;
    PBYTE  mem;

    /* The classes first: a class method's token is read from its
     * introducer's ClassData, which the class's making fills (C leaves the
     * order of _wpclsNew( _Card, ... )'s parts open). */
    NoteNewClass( Note_MajorVersion, Note_MinorVersion );
    CardNewClass( Card_MajorVersion, Card_MinorVersion );
    WPDataFileNewClass( WPDataFile_MajorVersion, WPDataFile_MinorVersion );
    WPTransientNewClass( WPTransient_MajorVersion, WPTransient_MinorVersion );
    if( argc > 1 && strcmp( argv[1], "save" ) == 0 ) return( save_mode() );
    if( argc > 1 && strcmp( argv[1], "load" ) == 0 ) return( load_mode() );
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

    abstract_checks();
    create_checks();
    file_checks();

    printf( "wps_object_test: done %d/%d\n", passed, total );
    return( passed == total ? 0 : 1 );
}
