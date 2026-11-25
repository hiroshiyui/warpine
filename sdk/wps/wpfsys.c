/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * wpfsys.c - WPFileSystem and M_WPFileSystem (ADR-0129): the objects of
 * files and directories, their state in the file's extended attributes.
 * The code is Warpine's own.
 *
 * - The object knows its file by its fully qualified name; the awake ones
 *   are listed by it (any case), so a file has one object.
 * - Its title is the file's name, or its .LONGNAME EA when the two differ
 *   (a title the file system cannot hold as a name); its types are its
 *   .TYPE EA (EAT_MVMT of EAT_ASCII); the rest of its state is a record
 *   (wpsint.h) in Warpine's own EA, WARPINE.WPSTATE.
 * - Changing the title of an initialized object renames the file (to the
 *   title, its unusable characters as '_'); wpFree deletes the file.
 */
#define INCL_WINWORKPLACE
#define INCL_DOSFILEMGR
#define INCL_DOSERRORS
#include <os2.h>
#include <string.h>
#include "wpfsys.ih"
#include "wpdataf.h"
#include "wpsint.h"

/* Warpine's EA for the rest of an object's state. */
#define STATE_EA     "WARPINE.WPSTATE"
#define LONGNAME_EA  ".LONGNAME"
#define TYPE_EA      ".TYPE"
/* The longest qualified name, its NUL included. */
#define MAX_PATH     260
/* An EA value's largest size (cbValue is 16 bits). */
#define MAX_EA_VALUE 0xFFFFUL
/* The most types .TYPE keeps. */
#define MAX_TYPES    64

/* ---- The awake objects, by file ---- */

typedef struct WrpFsAwake {
    struct WrpFsAwake *next;
    WPFileSystem      *obj;
} WrpFsAwake;

static WrpFsAwake *awake_files;

static int same_path( const char *a, const char *b )
{
    for( ;; a++, b++ ) {
        char x = *a, y = *b;
        if( x >= 'a' && x <= 'z' ) x = (char)( x - 'a' + 'A' );
        if( y >= 'a' && y <= 'z' ) y = (char)( y - 'a' + 'A' );
        if( x != y ) return( 0 );
        if( !x ) return( 1 );
    }
}

static WPFileSystem *awake_for( const char *path )
{
    WrpFsAwake *a;
    for( a = awake_files; a; a = a->next ) {
        PSZ name = WPFileSystemGetData( a->obj )->realName;
        if( name && same_path( name, path ) ) return( a->obj );
    }
    return( 0 );
}

static void forget( WPFileSystem *obj )
{
    WrpFsAwake **link = &awake_files;
    while( *link ) {
        if( ( *link )->obj == obj ) { WrpFsAwake *a = *link; *link = a->next; SOMFree( a ); }
        else link = &( *link )->next;
    }
}

static char *copy_string( const char *s )
{
    size_t n = strlen( s ) + 1;
    char  *p = (char *)SOMMalloc( n );
    if( p ) memcpy( p, s, n );
    return( p );
}

/* The last part of a qualified name. */
static const char *last_part( const char *path )
{
    const char *p = strrchr( path, '\\' );
    return( p ? p + 1 : path );
}

/* ---- Extended attributes, through the Dos calls ---- */

/* EA name's value (SOMMalloc'd, at *len); NULL when the file has none. */
static unsigned char *read_ea( PSZ path, const char *name, ULONG *len )
{
    size_t         n = strlen( name );
    ULONG          cap = 4 + 8 + 256 + MAX_EA_VALUE + 8;
    unsigned char *gea = (unsigned char *)SOMMalloc( 4 + 5 + n + 1 + 3 );
    unsigned char *fea = (unsigned char *)SOMMalloc( cap );
    unsigned char *value = 0;
    EAOP2          op;
    if( gea && fea ) {
        ULONG list = (ULONG)( 4 + 5 + n + 1 ), zero = 0;
        memcpy( gea, &list, 4 );
        memcpy( gea + 4, &zero, 4 );
        gea[8] = (unsigned char)n;
        memcpy( gea + 9, name, n + 1 );
        memcpy( fea, &cap, 4 );
        op.fpGEA2List = (PGEA2LIST)gea;
        op.fpFEA2List = (PFEA2LIST)fea;
        op.oError = 0;
        if( DosQueryPathInfo( path, FIL_QUERYEASFROMLIST, &op, sizeof( op ) ) == NO_ERROR ) {
            FEA2  *f = op.fpFEA2List->list;
            ULONG  size = f->cbValue;
            if( size && ( value = (unsigned char *)SOMMalloc( size ) ) ) {
                memcpy( value, (unsigned char *)f->szName + f->cbName + 1, size );
                *len = size;
            }
        }
    }
    if( gea ) SOMFree( gea );
    if( fea ) SOMFree( fea );
    return( value );
}

/* EA name set to value (len bytes; 0 deletes it). */
static BOOL write_ea( PSZ path, const char *name, const void *value, ULONG len )
{
    size_t         n = strlen( name );
    ULONG          size = (ULONG)( 4 + 8 + n + 1 + len );
    unsigned char *list;
    EAOP2          op;
    APIRET         rc;
    if( len > MAX_EA_VALUE || n > 255 || !( list = (unsigned char *)SOMMalloc( size ) ) ) return( FALSE );
    memset( list, 0, 12 );
    memcpy( list, &size, 4 );
    list[9] = (unsigned char)n;
    list[10] = (unsigned char)( len & 0xFF );
    list[11] = (unsigned char)( len >> 8 );
    memcpy( list + 12, name, n + 1 );
    if( len ) memcpy( list + 12 + n + 1, value, len );
    op.fpGEA2List = 0;
    op.fpFEA2List = (PFEA2LIST)list;
    op.oError = 0;
    rc = DosSetPathInfo( path, FIL_QUERYEASIZE, &op, sizeof( op ), 0 );
    SOMFree( list );
    return( rc == NO_ERROR );
}

/* An EAT_ASCII value's text (NUL-terminated into buf); FALSE otherwise. */
static BOOL ascii_value( const unsigned char *v, ULONG len, char *buf, ULONG cap )
{
    USHORT type, n;
    if( len < 4 ) return( FALSE );
    memcpy( &type, v, 2 );
    memcpy( &n, v + 2, 2 );
    if( type != EAT_ASCII || (ULONG)n + 4 > len || n >= cap ) return( FALSE );
    memcpy( buf, v + 4, n );
    buf[n] = '\0';
    return( TRUE );
}

/* The title the file gives: .LONGNAME, else its name. */
static void file_title( WPFileSystem *somSelf, char *title )
{
    PSZ            path = WPFileSystemGetData( somSelf )->realName;
    ULONG          len = 0;
    unsigned char *v = read_ea( path, LONGNAME_EA, &len );
    if( !v || !ascii_value( v, len, title, WP_MAX_TITLE ) ) {
        strncpy( title, last_part( path ), WP_MAX_TITLE - 1 );
        title[WP_MAX_TITLE - 1] = '\0';
    }
    if( v ) SOMFree( v );
}

static BOOL query_status( PSZ path, FILESTATUS3 *st )
{
    return( path && DosQueryPathInfo( path, FIL_STANDARD, st, sizeof( *st ) ) == NO_ERROR );
}

/* ---- WPFileSystem ---- */

SOM_Scope BOOL SOMLINK wpfs_wpQueryRealName( WPFileSystem *somSelf, PSZ pszFilename, PULONG pcb, BOOL fQualified )
{
    WPFileSystemData *somThis = WPFileSystemGetData( somSelf );
    const char       *name;
    ULONG             need;
    if( !_realName || !pcb ) return( FALSE );
    name = fQualified ? _realName : last_part( _realName );
    need = (ULONG)strlen( name ) + 1;
    if( !pszFilename ) { *pcb = need; return( TRUE ); }
    if( *pcb < need ) { *pcb = need; return( FALSE ); }
    memcpy( pszFilename, name, need );
    *pcb = need;
    return( TRUE );
}

SOM_Scope PSZ SOMLINK wpfs_wpQueryFilename( WPFileSystem *somSelf, PSZ pszFilename, BOOL fQualified )
{
    ULONG cb = MAX_PATH;
    if( !pszFilename ) return( 0 );
    if( !_wpQueryRealName( somSelf, pszFilename, &cb, fQualified ) ) return( 0 );
    return( pszFilename );
}

/* Renames the file to pszName, in its directory. */
SOM_Scope BOOL SOMLINK wpfs_wpSetRealName( WPFileSystem *somSelf, PSZ pszName )
{
    WPFileSystemData *somThis = WPFileSystemGetData( somSelf );
    char              path[MAX_PATH], *copy;
    size_t            dir, n;
    if( !_realName || !pszName || !*pszName || strpbrk( pszName, "\\/:*?\"<>|" ) ) return( FALSE );
    dir = (size_t)( last_part( _realName ) - _realName );
    n = strlen( pszName );
    if( dir + n >= sizeof( path ) ) return( FALSE );
    memcpy( path, _realName, dir );
    memcpy( path + dir, pszName, n + 1 );
    if( strcmp( path, _realName ) == 0 ) return( TRUE );
    if( DosMove( _realName, path ) != NO_ERROR || !( copy = copy_string( path ) ) ) return( FALSE );
    SOMFree( _realName );
    _realName = copy;
    return( TRUE );
}

SOM_Scope ULONG SOMLINK wpfs_wpQueryAttr( WPFileSystem *somSelf )
{
    FILESTATUS3 st;
    return( query_status( WPFileSystemGetData( somSelf )->realName, &st ) ? st.attrFile : 0 );
}

/* The attributes set; the dates are left as they are (zeros). */
SOM_Scope BOOL SOMLINK wpfs_wpSetAttr( WPFileSystem *somSelf, ULONG attrFile )
{
    FILESTATUS3 st;
    memset( &st, 0, sizeof( st ) );
    st.attrFile = attrFile;
    return( DosSetPathInfo( WPFileSystemGetData( somSelf )->realName, FIL_STANDARD, &st, sizeof( st ), 0 ) == NO_ERROR );
}

SOM_Scope ULONG SOMLINK wpfs_wpQueryFileSize( WPFileSystem *somSelf )
{
    FILESTATUS3 st;
    return( query_status( WPFileSystemGetData( somSelf )->realName, &st ) ? st.cbFile : 0 );
}

SOM_Scope ULONG SOMLINK wpfs_wpQueryEASize( WPFileSystem *somSelf )
{
    struct { FILESTATUS3 st; ULONG cbList; } info;
    PSZ path = WPFileSystemGetData( somSelf )->realName;
    if( !path || DosQueryPathInfo( path, FIL_QUERYEASIZE, &info, sizeof( info ) ) != NO_ERROR ) return( 0 );
    return( info.cbList );
}

/* One of the file's dates (which: 0 creation, 1 access, 2 write); 0, or
 * the error. */
static ULONG file_date( WPFileSystem *somSelf, int which, PFDATE fdate, PFTIME ftime )
{
    FILESTATUS3 st;
    PSZ         path = WPFileSystemGetData( somSelf )->realName;
    APIRET      rc;
    if( !path ) return( ERROR_FILE_NOT_FOUND );
    if( ( rc = DosQueryPathInfo( path, FIL_STANDARD, &st, sizeof( st ) ) ) != NO_ERROR ) return( rc );
    if( fdate ) *fdate = which == 0 ? st.fdateCreation : which == 1 ? st.fdateLastAccess : st.fdateLastWrite;
    if( ftime ) *ftime = which == 0 ? st.ftimeCreation : which == 1 ? st.ftimeLastAccess : st.ftimeLastWrite;
    return( NO_ERROR );
}

SOM_Scope ULONG SOMLINK wpfs_wpQueryCreation( WPFileSystem *somSelf, PFDATE fdate, PFTIME ftime )
{
    return( file_date( somSelf, 0, fdate, ftime ) );
}

SOM_Scope ULONG SOMLINK wpfs_wpQueryLastAccess( WPFileSystem *somSelf, PFDATE fdate, PFTIME ftime )
{
    return( file_date( somSelf, 1, fdate, ftime ) );
}

SOM_Scope ULONG SOMLINK wpfs_wpQueryLastWrite( WPFileSystem *somSelf, PFDATE fdate, PFTIME ftime )
{
    return( file_date( somSelf, 2, fdate, ftime ) );
}

/* .TYPE's types, joined by '\n' (read once, kept until wpSetType). */
SOM_Scope PSZ SOMLINK wpfs_wpQueryType( WPFileSystem *somSelf )
{
    WPFileSystemData *somThis = WPFileSystemGetData( somSelf );
    ULONG             len = 0, at, i;
    USHORT            type, count;
    unsigned char    *v;
    char             *out;
    if( _types ) return( _types );
    if( !( v = read_ea( _realName, TYPE_EA, &len ) ) ) return( "" );
    if( !( out = (char *)SOMMalloc( len + 1 ) ) ) { SOMFree( v ); return( "" ); }
    out[0] = '\0';
    memcpy( &type, v, len >= 2 ? 2 : 0 );
    if( len >= 6 && type == EAT_MVMT ) {
        memcpy( &count, v + 4, 2 );
        for( i = 0, at = 6; i < count && i < MAX_TYPES; i++ ) {
            char   text[WP_MAX_TITLE];
            USHORT n = 0;
            if( at + 4 > len || !ascii_value( v + at, len - at, text, sizeof( text ) ) ) break;
            memcpy( &n, v + at + 2, 2 );
            if( out[0] ) strcat( out, "\n" );
            strcat( out, text );
            at += 4 + n;
        }
    } else {
        ascii_value( v, len, out, len + 1 );
    }
    SOMFree( v );
    _types = out;
    return( _types );
}

/* .TYPE as EAT_MVMT of EAT_ASCII ("" removes it); pfeal's EAs too. */
SOM_Scope BOOL SOMLINK wpfs_wpSetType( WPFileSystem *somSelf, PSZ pszTypes, PFEA2LIST pfeal )
{
    WPFileSystemData *somThis = WPFileSystemGetData( somSelf );
    size_t            n = pszTypes ? strlen( pszTypes ) : 0;
    unsigned char    *v = (unsigned char *)SOMMalloc( 6 + n + 4 * MAX_TYPES );
    ULONG             at = 6;
    USHORT            count = 0, word;
    const char       *p = pszTypes;
    BOOL              ok;
    if( !v || !_realName ) { if( v ) SOMFree( v ); return( FALSE ); }
    while( p && *p && count < MAX_TYPES ) {
        const char *end = strchr( p, '\n' );
        USHORT      len = (USHORT)( end ? (size_t)( end - p ) : strlen( p ) );
        if( len ) {
            word = EAT_ASCII; memcpy( v + at, &word, 2 );
            memcpy( v + at + 2, &len, 2 );
            memcpy( v + at + 4, p, len );
            at += 4 + len;
            count++;
        }
        p = end ? end + 1 : 0;
    }
    word = EAT_MVMT; memcpy( v, &word, 2 );
    word = 0;        memcpy( v + 2, &word, 2 );
    memcpy( v + 4, &count, 2 );
    ok = write_ea( _realName, TYPE_EA, v, count ? at : 0 );
    SOMFree( v );
    if( _types ) { SOMFree( _types ); _types = 0; }
    if( ok && pfeal ) {
        EAOP2 op;
        op.fpGEA2List = 0;
        op.fpFEA2List = pfeal;
        op.oError = 0;
        ok = DosSetPathInfo( _realName, FIL_QUERYEASIZE, &op, sizeof( op ), 0 ) == NO_ERROR;
    }
    return( ok );
}

/* The file renamed to the title (unusable characters as '_'), then the
 * title set (saved: .LONGNAME when it differs from the name). */
SOM_Scope BOOL SOMLINK wpfs_wpSetTitleAndRenameFile( WPFileSystem *somSelf, PSZ pszNewTitle, ULONG fConfirmations )
{
    char   name[WP_MAX_TITLE];
    size_t i, n;
    (void)fConfirmations;
    if( !pszNewTitle || !*pszNewTitle || ( n = strlen( pszNewTitle ) ) >= WP_MAX_TITLE ) return( FALSE );
    for( i = 0; i < n; i++ ) {
        char c = pszNewTitle[i];
        name[i] = ( c < ' ' || strchr( "\\/:*?\"<>|", c ) ) ? '_' : c;
    }
    name[n] = '\0';
    if( !_wpSetRealName( somSelf, name ) ) return( FALSE );
    return( WPFileSystem_parent_WPObject_wpSetTitle( somSelf, pszNewTitle ) );
}

/* A title change on an initialized object renames the file; while the
 * object is made or restored, it only sets the title. */
SOM_Scope BOOL SOMLINK wpfs_wpSetTitle( WPFileSystem *somSelf, PSZ pszNewTitle )
{
    if( !_wpIsObjectInitialized( somSelf ) ) return( WPFileSystem_parent_WPObject_wpSetTitle( somSelf, pszNewTitle ) );
    return( _wpSetTitleAndRenameFile( somSelf, pszNewTitle, 0 ) );
}

/* Every drive keeps extended attributes. */
SOM_Scope ULONG SOMLINK wpfs_wpQueryEASupport( WPFileSystem *somSelf )
{
    (void)somSelf;
    return( NO_ERROR );
}

SOM_Scope BOOL SOMLINK wpfs_wpRefresh( WPFileSystem *somSelf, ULONG ulView, PVOID pReserved )
{
    WPFileSystemData *somThis = WPFileSystemGetData( somSelf );
    char              title[WP_MAX_TITLE];
    (void)ulView;
    (void)pReserved;
    if( !query_status( _realName, &(FILESTATUS3){ 0 } ) ) return( FALSE );
    if( _types ) { SOMFree( _types ); _types = 0; }
    file_title( somSelf, title );
    return( WPFileSystem_parent_WPObject_wpSetTitle( somSelf, title ) );
}

/* WPObject's state, then the title the file gives. */
SOM_Scope BOOL SOMLINK wpfs_wpRestoreState( WPFileSystem *somSelf, ULONG ulReserved )
{
    char title[WP_MAX_TITLE];
    if( !WPFileSystem_parent_WPObject_wpRestoreState( somSelf, ulReserved ) ) return( FALSE );
    file_title( somSelf, title );
    return( WPFileSystem_parent_WPObject_wpSetTitle( somSelf, title ) );
}

/* The state record in WARPINE.WPSTATE, the title in .LONGNAME when it is
 * not the file's name. */
SOM_Scope BOOL SOMLINK wpfs_wpSaveImmediate( WPFileSystem *somSelf )
{
    WPFileSystemData *somThis = WPFileSystemGetData( somSelf );
    unsigned char    *rec;
    ULONG             size;
    PSZ               title;
    BOOL              ok;
    if( !_realName || !WPFileSystem_parent_WPObject_wpSaveImmediate( somSelf ) ) return( FALSE );
    if( !( rec = wrp_wps_record_make( somSelf, &size ) ) ) return( FALSE );
    ok = size <= MAX_EA_VALUE && write_ea( _realName, STATE_EA, rec, size );
    SOMFree( rec );
    title = _wpQueryTitle( somSelf );
    if( ok && strcmp( title, last_part( _realName ) ) != 0 ) {
        size_t        n = strlen( title );
        unsigned char v[4 + WP_MAX_TITLE];
        USHORT        word = EAT_ASCII, len = (USHORT)n;
        memcpy( v, &word, 2 );
        memcpy( v + 2, &len, 2 );
        memcpy( v + 4, title, n );
        ok = write_ea( _realName, LONGNAME_EA, v, (ULONG)( 4 + n ) );
    } else if( ok ) {
        write_ea( _realName, LONGNAME_EA, 0, 0 );
    }
    return( ok );
}

/* The file (or empty directory) deleted first, then the object. */
SOM_Scope BOOL SOMLINK wpfs_wpFree( WPFileSystem *somSelf )
{
    WPFileSystemData *somThis = WPFileSystemGetData( somSelf );
    FILESTATUS3       st;
    if( _realName && query_status( _realName, &st ) ) {
        APIRET rc = ( st.attrFile & FILE_DIRECTORY ) ? DosDeleteDir( _realName ) : DosDelete( _realName );
        if( rc != NO_ERROR ) return( FALSE );
    }
    return( WPFileSystem_parent_WPObject_wpFree( somSelf ) );
}

SOM_Scope void SOMLINK wpfs_wpUnInitData( WPFileSystem *somSelf )
{
    WPFileSystemData *somThis = WPFileSystemGetData( somSelf );
    forget( somSelf );
    if( _realName ) SOMFree( _realName );
    if( _types ) SOMFree( _types );
    _realName = _types = 0;
    WPFileSystem_parent_WPObject_wpUnInitData( somSelf );
}

/* ---- M_WPFileSystem ---- */

/* The class a file's object is of: the one its state names (registered, a
 * WPFileSystem), else WPFolder for a directory (when there is one),
 * WPDataFile for a file. */
static SOMClass *class_for( const char *name, BOOL directory )
{
    SOMClass *cls = 0;
    if( name ) cls = _somFindClass( SOMClassMgrObject, somIdFromString( (char *)name ), 0, 0 );
    if( cls && _somDescendedFrom( cls, _WPFileSystem ) ) return( cls );
    if( directory ) return( _somFindClass( SOMClassMgrObject, somIdFromString( "WPFolder" ), 0, 0 ) );
    return( _WPDataFile );
}

SOM_Scope WPObject *SOMLINK wpfsM_wpclsQueryObjectFromPath( M_WPFileSystem *somSelf, PSZ pszFQPath )
{
    char                 path[MAX_PATH], name[WP_MAX_TITLE];
    FILESTATUS3          st;
    WPFileSystem        *obj;
    SOMClass            *cls;
    unsigned char       *rec;
    const unsigned char *part = 0;
    ULONG                size = 0, part_len = 0;
    BOOL                 named;
    if( !pszFQPath || !*pszFQPath ) return( 0 );
    /* An object ID names a stored object. */
    if( pszFQPath[0] == '<' ) return( _wpclsQueryObject( somSelf, WinQueryObject( pszFQPath ) ) );
    if( DosQueryPathInfo( pszFQPath, FIL_QUERYFULLNAME, path, sizeof( path ) ) != NO_ERROR ) return( 0 );
    if( ( obj = awake_for( path ) ) ) { _wpLockObject( obj ); return( obj ); }
    if( !query_status( path, &st ) ) return( 0 );
    rec = read_ea( path, STATE_EA, &size );
    named = rec && wrp_wps_record_parse( rec, size, name, &part, &part_len );
    cls = class_for( named ? name : 0, ( st.attrFile & FILE_DIRECTORY ) != 0 );
    obj = cls ? (WPFileSystem *)_somNew( cls ) : 0;
    if( obj ) {
        WrpFsAwake *a = (WrpFsAwake *)SOMMalloc( sizeof( *a ) );
        WPFileSystemData *d = WPFileSystemGetData( obj );
        d->realName = copy_string( path );
        if( a ) { a->obj = obj; a->next = awake_files; awake_files = a; }
        if( !a || !d->realName || ( named && !wrp_wps_store_read( obj, part, part_len ) ) || !_wpRestoreState( obj, 0 ) ) {
            _somFree( obj );
            obj = 0;
        } else {
            wrp_wps_set_initialized( obj );
            _wpLockObject( obj );
        }
    }
    if( rec ) SOMFree( rec );
    return( obj );
}

/* A new file needs its folder: that comes with WPFolder; until then no
 * file object is made this way (Warpine's limit). */
SOM_Scope WPObject *SOMLINK wpfsM_wpclsNew( M_WPFileSystem *somSelf, PSZ pszTitle, PSZ pszSetupEnv,
                                            WPFolder *Folder, BOOL fLock )
{
    (void)somSelf; (void)pszTitle; (void)pszSetupEnv; (void)Folder; (void)fLock;
    return( 0 );
}
