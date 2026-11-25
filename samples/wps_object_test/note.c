/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * note.c - Note and M_Note (note.idl): a WPObject subclass as a program
 * writes one, through somidl's bindings and the SDK's libwps (ADR-0129).
 */
#define INCL_WINWORKPLACE
#include <os2.h>
#include <string.h>
#include "note.ih"

int note_inits, note_uninits, note_class_inits;

#define NOTE_KEY_TEXT 1

static void set_text( Note *somSelf, const char *text )
{
    NoteData *somThis = NoteGetData( somSelf );
    if( _text ) _wpFreeMem( somSelf, (PBYTE)_text );
    _text = (PSZ)_wpAllocMem( somSelf, strlen( text ) + 1, NULL );
    if( _text ) strcpy( _text, text );
}

SOM_Scope PSZ SOMLINK note_queryText( Note *somSelf )
{
    NoteData *somThis = NoteGetData( somSelf );
    return( _text ? _text : (PSZ)"" );
}

/* The parent first, as the Reference asks. */
SOM_Scope void SOMLINK note_wpInitData( Note *somSelf )
{
    NoteData *somThis = NoteGetData( somSelf );
    Note_parent_WPObject_wpInitData( somSelf );
    _text = 0;
    note_inits++;
}

/* The parent last; the memory of wpAllocMem goes with the object anyway. */
SOM_Scope void SOMLINK note_wpUnInitData( Note *somSelf )
{
    note_uninits++;
    Note_parent_WPObject_wpUnInitData( somSelf );
}

SOM_Scope BOOL SOMLINK note_wpSetup( Note *somSelf, PSZ pszSetupString )
{
    char  text[128];
    ULONG cb = sizeof( text );
    if( !Note_parent_WPObject_wpSetup( somSelf, pszSetupString ) ) return( FALSE );
    if( _wpScanSetupString( somSelf, pszSetupString, "TEXT", text, &cb ) ) set_text( somSelf, text );
    return( TRUE );
}

SOM_Scope BOOL SOMLINK note_wpSaveState( Note *somSelf )
{
    NoteData *somThis = NoteGetData( somSelf );
    return( Note_parent_WPObject_wpSaveState( somSelf )
            && _wpSaveString( somSelf, "Note", NOTE_KEY_TEXT, _text ? _text : (PSZ)"" ) );
}

SOM_Scope BOOL SOMLINK note_wpRestoreState( Note *somSelf, ULONG ulReserved )
{
    char  text[128];
    ULONG cb = sizeof( text );
    if( !Note_parent_WPObject_wpRestoreState( somSelf, ulReserved ) ) return( FALSE );
    if( !_wpRestoreString( somSelf, "Note", NOTE_KEY_TEXT, text, &cb ) ) return( FALSE );
    set_text( somSelf, text );
    return( TRUE );
}

SOM_Scope PSZ SOMLINK noteM_wpclsQueryTitle( M_Note *somSelf )
{
    (void)somSelf;
    return( "Note pad" );
}

/* Notes cannot be printed: the class's style is the objects' first. */
SOM_Scope ULONG SOMLINK noteM_wpclsQueryStyle( M_Note *somSelf )
{
    return( M_Note_parent_M_WPObject_wpclsQueryStyle( somSelf ) | CLSSTYLE_NEVERPRINT );
}

SOM_Scope void SOMLINK noteM_wpclsInitData( M_Note *somSelf )
{
    M_Note_parent_M_WPObject_wpclsInitData( somSelf );
    note_class_inits++;
}
