/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * wpfolder.c - WPFolder and M_WPFolder (ADR-0129): a directory's object
 * and its content list. The code is Warpine's own.
 *
 * - The content list holds objects, in the order they came: wpPopulate's
 *   (the directory's entries, each locked as the Reference says) and those
 *   wpAddToContent added. An object leaves it when it is deleted, made
 *   dormant or taken out (wpDeleteFromContent); one wpPopulate locked is
 *   unlocked then.
 * - wpFree deletes the contents first (wpDeleteContents, as the Reference
 *   says), then the directory.
 */
#define INCL_WINWORKPLACE
#define INCL_DOSFILEMGR
#define INCL_DOSERRORS
#include <os2.h>
#include <string.h>
#include "wpfolder.ih"
#include "wpsint.h"

/* The longest qualified name, its NUL included. */
#define MAX_PATH 260

typedef struct WrpFolderItem {
    struct WrpFolderItem *next;
    struct WrpFolderItem *prev;
    WPObject             *obj;
    BOOL                  locked;
} WrpFolderItem;

static WrpFolderItem *find_item( WPFolderData *somThis, WPObject *obj )
{
    WrpFolderItem *i;
    for( i = (WrpFolderItem *)_first; i; i = i->next ) if( i->obj == obj ) return( i );
    return( 0 );
}

static BOOL add_item( WPFolderData *somThis, WPObject *obj, BOOL locked )
{
    WrpFolderItem *i;
    if( !obj || find_item( somThis, obj ) ) return( FALSE );
    if( !( i = (WrpFolderItem *)SOMMalloc( sizeof( *i ) ) ) ) return( FALSE );
    i->obj = obj;
    i->locked = locked;
    i->next = 0;
    i->prev = (WrpFolderItem *)_last;
    if( _last ) ( (WrpFolderItem *)_last )->next = i;
    else _first = i;
    _last = i;
    return( TRUE );
}

static void drop_item( WPFolderData *somThis, WrpFolderItem *i )
{
    if( i->prev ) i->prev->next = i->next;
    else _first = i->next;
    if( i->next ) i->next->prev = i->prev;
    else _last = i->prev;
    if( i->locked ) _wpUnlockObject( i->obj );
    SOMFree( i );
}

/* The directory's entries, one call per entry (its qualified name and
 * whether it is a directory); `.` and `..` left out. */
static void each_entry( WPFolder *somSelf, void ( *f )( WPFolder *, const char *, BOOL, void * ), void *arg )
{
    char         dir[MAX_PATH], pattern[MAX_PATH + 2], path[MAX_PATH];
    ULONG        cb = sizeof( dir ), n = 1;
    size_t       len;
    HDIR         h = HDIR_CREATE;
    FILEFINDBUF3 fb;
    APIRET       rc;
    if( !_wpQueryRealName( somSelf, dir, &cb, TRUE ) ) return;
    len = strlen( dir );
    if( len && dir[len - 1] == '\\' ) dir[--len] = '\0';
    if( len + 3 > sizeof( pattern ) ) return;
    memcpy( pattern, dir, len );
    memcpy( pattern + len, "\\*", 3 );
    rc = DosFindFirst( pattern, &h, FILE_DIRECTORY | FILE_HIDDEN | FILE_SYSTEM | FILE_READONLY | FILE_ARCHIVED,
                       &fb, sizeof( fb ), &n, FIL_STANDARD );
    while( rc == NO_ERROR && n ) {
        size_t name = strlen( fb.achName );
        if( strcmp( fb.achName, "." ) != 0 && strcmp( fb.achName, ".." ) != 0 && len + 1 + name < sizeof( path ) ) {
            memcpy( path, dir, len );
            path[len] = '\\';
            memcpy( path + len + 1, fb.achName, name + 1 );
            f( somSelf, path, ( fb.attrFile & FILE_DIRECTORY ) != 0, arg );
        }
        n = 1;
        rc = DosFindNext( h, &fb, sizeof( fb ), &n );
    }
    if( h != HDIR_CREATE ) DosFindClose( h );
}

/* ---- WPFolder ---- */

struct populating { BOOL folders_only; };

static void populate_one( WPFolder *somSelf, const char *path, BOOL directory, void *arg )
{
    WPFolderData *somThis = WPFolderGetData( somSelf );
    WPObject     *obj;
    if( ( (struct populating *)arg )->folders_only && !directory ) return;
    /* The object comes locked; one already listed keeps its one lock. */
    if( !( obj = _wpclsQueryObjectFromPath( _WPFileSystem, (PSZ)path ) ) ) return;
    if( !add_item( somThis, obj, TRUE ) ) _wpUnlockObject( obj );
}

SOM_Scope BOOL SOMLINK wpfo_wpPopulate( WPFolder *somSelf, ULONG ulReserved, PSZ pszPath, BOOL fFoldersOnly )
{
    WPFolderData     *somThis = WPFolderGetData( somSelf );
    struct populating p;
    (void)ulReserved;
    (void)pszPath;
    if( _flags & FOI_POPULATEDWITHALL ) return( TRUE );
    if( fFoldersOnly && ( _flags & FOI_POPULATEDWITHFOLDERS ) ) return( TRUE );
    p.folders_only = fFoldersOnly;
    each_entry( somSelf, populate_one, &p );
    _flags |= fFoldersOnly ? FOI_POPULATEDWITHFOLDERS : ( FOI_POPULATEDWITHALL | FOI_POPULATEDWITHFOLDERS );
    return( TRUE );
}

SOM_Scope WPObject *SOMLINK wpfo_wpQueryContent( WPFolder *somSelf, WPObject *Object, ULONG ulOption )
{
    WPFolderData  *somThis = WPFolderGetData( somSelf );
    WrpFolderItem *i;
    switch( ulOption ) {
    case QC_FIRST: return( _first ? ( (WrpFolderItem *)_first )->obj : 0 );
    case QC_LAST:  return( _last ? ( (WrpFolderItem *)_last )->obj : 0 );
    case QC_NEXT:  return( ( i = find_item( somThis, Object ) ) && i->next ? i->next->obj : 0 );
    default:       return( 0 );
    }
}

SOM_Scope BOOL SOMLINK wpfo_wpAddToContent( WPFolder *somSelf, WPObject *Object )
{
    return( add_item( WPFolderGetData( somSelf ), Object, FALSE ) );
}

SOM_Scope BOOL SOMLINK wpfo_wpDeleteFromContent( WPFolder *somSelf, WPObject *Object )
{
    WPFolderData  *somThis = WPFolderGetData( somSelf );
    WrpFolderItem *i = find_item( somThis, Object );
    if( !i ) return( FALSE );
    drop_item( somThis, i );
    return( TRUE );
}

static void note_folder( WPFolder *somSelf, const char *path, BOOL directory, void *arg )
{
    (void)somSelf;
    (void)path;
    if( directory ) *(BOOL *)arg = TRUE;
}

SOM_Scope BOOL SOMLINK wpfo_wpContainsFolders( WPFolder *somSelf, PBOOL pfSubFolders )
{
    BOOL found = FALSE;
    if( !pfSubFolders ) return( FALSE );
    each_entry( somSelf, note_folder, &found );
    *pfSubFolders = found;
    return( TRUE );
}

/* Every object of the folder deleted (populated first); NO_DELETE when
 * one is refused, the others deleted all the same. */
SOM_Scope ULONG SOMLINK wpfo_wpDeleteContents( WPFolder *somSelf, ULONG fConfirmations )
{
    WPFolderData  *somThis = WPFolderGetData( somSelf );
    WrpFolderItem *i;
    WPObject     **objs;
    ULONG          count = 0, k, rc = OK_DELETE;
    _wpPopulate( somSelf, 0, 0, FALSE );
    for( i = (WrpFolderItem *)_first; i; i = i->next ) count++;
    if( !count ) return( OK_DELETE );
    if( !( objs = (WPObject **)SOMMalloc( count * sizeof( *objs ) ) ) ) return( NO_DELETE );
    for( k = 0, i = (WrpFolderItem *)_first; i; i = i->next ) objs[k++] = i->obj;
    for( k = 0; k < count; k++ ) {
        /* Deleting takes the object out of this list. */
        if( _wpDelete( objs[k], fConfirmations ) != OK_DELETE ) rc = NO_DELETE;
    }
    SOMFree( objs );
    return( rc );
}

SOM_Scope ULONG SOMLINK wpfo_wpQueryFldrFlags( WPFolder *somSelf )
{
    return( WPFolderGetData( somSelf )->flags );
}

/* The list let go; its objects stay awake (those wpPopulate locked are
 * unlocked). */
SOM_Scope void SOMLINK wpfo_wpUnInitData( WPFolder *somSelf )
{
    WPFolderData *somThis = WPFolderGetData( somSelf );
    while( _first ) drop_item( somThis, (WrpFolderItem *)_first );
    _flags = 0;
    WPFolder_parent_WPFileSystem_wpUnInitData( somSelf );
}

/* The contents first, then the directory. */
SOM_Scope BOOL SOMLINK wpfo_wpFree( WPFolder *somSelf )
{
    if( _wpDeleteContents( somSelf, 0 ) != OK_DELETE ) return( FALSE );
    return( WPFolder_parent_WPFileSystem_wpFree( somSelf ) );
}

/* ---- M_WPFolder ---- */

/* The title the Reference gives the class's template. */
SOM_Scope PSZ SOMLINK wpfoM_wpclsQueryTitle( M_WPFolder *somSelf )
{
    (void)somSelf;
    return( "Folder" );
}
