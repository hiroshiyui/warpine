/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * wpobject.c - WPObject and M_WPObject, the root Workplace object class
 * and its metaclass (ADR-0129), from the Workplace Shell Programming
 * Reference's descriptions; the code is Warpine's own.
 *
 * - An object is initialized (wpInitData) from somDefaultInit and
 *   uninitialized (wpUnInitData) from somDestruct, through Warpine's
 *   initializer protocol, so each runs once whichever class's somNew made
 *   it.
 * - wpSaveString / Long / Data keep what they are given with the object,
 *   by class name and key, for wpRestoreString / Long / Data; writing it
 *   to OS2.INI belongs to the storage classes (WPAbstract), still to come.
 * - wpAllocMem's blocks are listed with the object and freed with it.
 * - wpDelete asks wpConfirmDelete, which refuses for OBJSTYLE_NODELETE;
 *   there is no confirmation dialog yet.
 */
#define INCL_WINWORKPLACE
#include <os2.h>
#include <string.h>
#include "wpobject.ih"

/* What wpSave* kept: a list per object. */
typedef struct WrpWpSaved {
    struct WrpWpSaved *next;
    char              *cls;
    ULONG              key;
    ULONG              kind;
    ULONG              size;
    unsigned char      data[1];
} WrpWpSaved;

enum { SAVED_STRING = 1, SAVED_LONG, SAVED_DATA };

/* A block of wpAllocMem: its header, then the memory (8-aligned). */
typedef struct WrpWpMem {
    struct WrpWpMem *next;
    ULONG            pad;
} WrpWpMem;

/* The longest value wpSetup reads for one of WPObject's keys. */
#define SETUP_VALUE_MAX 260

static int same_key( const char *a, const char *b, size_t n )
{
    size_t i;
    for( i = 0; i < n; i++ ) {
        char x = a[i], y = b[i];
        if( x >= 'a' && x <= 'z' ) x = (char)( x - 'a' + 'A' );
        if( y >= 'a' && y <= 'z' ) y = (char)( y - 'a' + 'A' );
        if( x != y || y == '\0' ) return( 0 );
    }
    return( b[n] == '\0' );
}

static char *copy_string( const char *s )
{
    size_t n = strlen( s ) + 1;
    char  *p = (char *)SOMMalloc( n );
    if( p ) memcpy( p, s, n );
    return( p );
}

/* ---- WPObject: setup ---- */

SOM_Scope BOOL SOMLINK wpobj_wpScanSetupString( WPObject *somSelf, PSZ pszSetupString, PSZ pszKey,
                                                PSZ pszValue, PULONG pcbValue )
{
    const char *p = pszSetupString;
    size_t      keylen;
    (void)somSelf;
    if( !p || !pszKey || !pcbValue ) return( FALSE );
    keylen = strlen( pszKey );
    while( *p ) {
        const char *name = p, *eq = 0;
        /* One "NAME=value" item, up to an unescaped ';'. */
        while( *p && *p != ';' ) {
            if( *p == '^' && p[1] == ';' ) p++;
            else if( *p == '=' && !eq ) eq = p;
            p++;
        }
        if( eq && (size_t)( eq - name ) == keylen && same_key( name, pszKey, keylen ) ) {
            const char *v;
            ULONG       need = 1, i = 0;
            for( v = eq + 1; v < p; v++ ) { if( *v == '^' && v[1] == ';' ) v++; need++; }
            if( !pszValue ) { *pcbValue = need; return( TRUE ); }
            if( *pcbValue < need ) { *pcbValue = need; return( FALSE ); }
            for( v = eq + 1; v < p; v++ ) {
                if( *v == '^' && v[1] == ';' ) v++;
                pszValue[i++] = *v;
            }
            pszValue[i] = '\0';
            *pcbValue = need;
            return( TRUE );
        }
        if( *p == ';' ) p++;
    }
    return( FALSE );
}

/* The value of key in setup, in buf; FALSE when there is none. */
static BOOL setup_value( WPObject *somSelf, PSZ setup, const char *key, char *buf )
{
    ULONG cb = SETUP_VALUE_MAX;
    return( _wpScanSetupString( somSelf, setup, (PSZ)key, buf, &cb ) );
}

static const struct { const char *key; ULONG style; } style_keys[] = {
    { "NOCOPY", OBJSTYLE_NOCOPY },         { "NODELETE", OBJSTYLE_NODELETE },
    { "NODRAG", OBJSTYLE_NODRAG },         { "NODROP", OBJSTYLE_NODROP },
    { "NOLINK", OBJSTYLE_NOLINK },         { "NOSHADOW", OBJSTYLE_NOLINK },
    { "NOMOVE", OBJSTYLE_NOMOVE },         { "NOPRINT", OBJSTYLE_NOPRINT },
    { "NORENAME", OBJSTYLE_NORENAME },     { "NOSETTINGS", OBJSTYLE_NOSETTINGS },
    { "NOTVISIBLE", OBJSTYLE_NOTVISIBLE }, { "TEMPLATE", OBJSTYLE_TEMPLATE },
    { "LOCKEDINPLACE", OBJSTYLE_LOCKEDINPLACE },
};

/* WPObject's keys: TITLE, OBJECTID, DEFAULTVIEW and the style ones; the
 * others the Reference lists (views, icons, help, menus) are accepted
 * and not used yet. */
SOM_Scope BOOL SOMLINK wpobj_wpSetup( WPObject *somSelf, PSZ pszSetupString )
{
    char   value[SETUP_VALUE_MAX];
    size_t i;
    if( !pszSetupString ) return( TRUE );
    if( setup_value( somSelf, pszSetupString, "TITLE", value ) && !_wpSetTitle( somSelf, value ) ) return( FALSE );
    if( setup_value( somSelf, pszSetupString, "OBJECTID", value ) && !_wpSetObjectID( somSelf, value ) ) return( FALSE );
    if( setup_value( somSelf, pszSetupString, "DEFAULTVIEW", value ) ) {
        if( same_key( value, "SETTINGS", 8 ) ) _wpSetDefaultView( somSelf, OPEN_SETTINGS );
        else if( same_key( value, "DEFAULT", 7 ) ) _wpSetDefaultView( somSelf, OPEN_DEFAULT );
        else if( value[0] >= '0' && value[0] <= '9' && value[1] == '\0' )
            _wpSetDefaultView( somSelf, OPEN_USER + (ULONG)( value[0] - '0' ) );
    }
    for( i = 0; i < sizeof( style_keys ) / sizeof( style_keys[0] ); i++ ) {
        ULONG style;
        if( !setup_value( somSelf, pszSetupString, style_keys[i].key, value ) ) continue;
        style = _wpQueryStyle( somSelf );
        if( same_key( value, "YES", 3 ) ) _wpSetStyle( somSelf, style | style_keys[i].style );
        else if( same_key( value, "NO", 2 ) ) _wpSetStyle( somSelf, style & ~style_keys[i].style );
    }
    return( TRUE );
}

SOM_Scope BOOL SOMLINK wpobj_wpSetupOnce( WPObject *somSelf, PSZ pszSetupString )
{
    return( _wpSetup( somSelf, pszSetupString ) );
}

/* ---- WPObject: data ---- */

SOM_Scope void SOMLINK wpobj_wpInitData( WPObject *somSelf )
{
    WPObjectData *somThis = WPObjectGetData( somSelf );
    _style = _wpclsQueryStyle( (M_WPObject *)_somGetClass( somSelf ) );
    _defaultView = OPEN_DEFAULT;
}

SOM_Scope void SOMLINK wpobj_wpUnInitData( WPObject *somSelf )
{
    WPObjectData *somThis = WPObjectGetData( somSelf );
    WrpWpSaved   *s = (WrpWpSaved *)_saved;
    WrpWpMem     *m = (WrpWpMem *)_memory;
    while( s ) { WrpWpSaved *next = s->next; SOMFree( s->cls ); SOMFree( s ); s = next; }
    while( m ) { WrpWpMem *next = m->next; SOMFree( m ); m = next; }
    _saved = _memory = 0;
    if( _title ) SOMFree( _title );
    if( _objectID ) SOMFree( _objectID );
    _title = _objectID = 0;
}

SOM_Scope BOOL SOMLINK wpobj_wpSetTitle( WPObject *somSelf, PSZ pszNewTitle )
{
    WPObjectData *somThis = WPObjectGetData( somSelf );
    char         *copy;
    if( !pszNewTitle || strlen( pszNewTitle ) >= WP_MAX_TITLE ) return( FALSE );
    if( !( copy = copy_string( pszNewTitle ) ) ) return( FALSE );
    if( _title ) SOMFree( _title );
    _title = copy;
    return( TRUE );
}

SOM_Scope PSZ SOMLINK wpobj_wpQueryTitle( WPObject *somSelf )
{
    WPObjectData *somThis = WPObjectGetData( somSelf );
    return( _title ? _title : (PSZ)"" );
}

SOM_Scope BOOL SOMLINK wpobj_wpSetStyle( WPObject *somSelf, ULONG ulNewStyle )
{
    WPObjectData *somThis = WPObjectGetData( somSelf );
    _style = ulNewStyle;
    return( TRUE );
}

SOM_Scope ULONG SOMLINK wpobj_wpQueryStyle( WPObject *somSelf )
{
    return( WPObjectGetData( somSelf )->style );
}

SOM_Scope BOOL SOMLINK wpobj_wpSetDefaultView( WPObject *somSelf, ULONG ulView )
{
    WPObjectData *somThis = WPObjectGetData( somSelf );
    _defaultView = ulView;
    return( TRUE );
}

/* The object's own view, else (OPEN_DEFAULT) its class's. */
SOM_Scope ULONG SOMLINK wpobj_wpQueryDefaultView( WPObject *somSelf )
{
    WPObjectData *somThis = WPObjectGetData( somSelf );
    if( _defaultView != OPEN_DEFAULT ) return( _defaultView );
    return( _wpclsQueryDefaultView( (M_WPObject *)_somGetClass( somSelf ) ) );
}

/* An object ID is "<NAME>". */
SOM_Scope BOOL SOMLINK wpobj_wpSetObjectID( WPObject *somSelf, PSZ pszObjectID )
{
    WPObjectData *somThis = WPObjectGetData( somSelf );
    size_t        n = pszObjectID ? strlen( pszObjectID ) : 0;
    char         *copy;
    if( n < 3 || n >= WP_MAX_TITLE || pszObjectID[0] != '<' || pszObjectID[n - 1] != '>' ) return( FALSE );
    if( !( copy = copy_string( pszObjectID ) ) ) return( FALSE );
    if( _objectID ) SOMFree( _objectID );
    _objectID = copy;
    return( TRUE );
}

SOM_Scope PSZ SOMLINK wpobj_wpQueryObjectID( WPObject *somSelf )
{
    return( WPObjectGetData( somSelf )->objectID );
}

SOM_Scope BOOL SOMLINK wpobj_wpSetError( WPObject *somSelf, ULONG ulErrorId )
{
    WPObjectGetData( somSelf )->error = ulErrorId;
    return( TRUE );
}

SOM_Scope ULONG SOMLINK wpobj_wpQueryError( WPObject *somSelf )
{
    return( WPObjectGetData( somSelf )->error );
}

/* ---- WPObject: memory ---- */

SOM_Scope PBYTE SOMLINK wpobj_wpAllocMem( WPObject *somSelf, ULONG cbBytes, PULONG prc )
{
    WPObjectData *somThis = WPObjectGetData( somSelf );
    WrpWpMem     *m = 0;
    if( cbBytes <= 0x7FFFFFFFUL - sizeof( WrpWpMem ) ) m = (WrpWpMem *)SOMMalloc( sizeof( WrpWpMem ) + cbBytes );
    if( !m ) {
        if( prc ) *prc = ERROR_NOT_ENOUGH_MEMORY;
        return( 0 );
    }
    m->next = (WrpWpMem *)_memory;
    _memory = m;
    if( prc ) *prc = NO_ERROR;
    return( (PBYTE)( m + 1 ) );
}

SOM_Scope BOOL SOMLINK wpobj_wpFreeMem( WPObject *somSelf, PBYTE pByte )
{
    WPObjectData *somThis = WPObjectGetData( somSelf );
    WrpWpMem    **link = (WrpWpMem **)&_memory;
    for( ; *link; link = &( *link )->next ) {
        if( (PBYTE)( *link + 1 ) == pByte ) {
            WrpWpMem *m = *link;
            *link = m->next;
            SOMFree( m );
            return( TRUE );
        }
    }
    return( FALSE );
}

/* ---- WPObject: save and restore ---- */

static WrpWpSaved **find_saved( WPObjectData *somThis, const char *cls, ULONG key )
{
    WrpWpSaved **link = (WrpWpSaved **)&_saved;
    for( ; *link; link = &( *link )->next )
        if( ( *link )->key == key && strcmp( ( *link )->cls, cls ) == 0 ) return( link );
    return( link );
}

static BOOL save( WPObject *somSelf, PSZ cls, ULONG key, ULONG kind, const void *data, ULONG size )
{
    WPObjectData *somThis = WPObjectGetData( somSelf );
    WrpWpSaved  **link, *s;
    if( !cls || ( size && !data ) || size > 0x7FFFFFFFUL - sizeof( WrpWpSaved ) ) return( FALSE );
    if( !( s = (WrpWpSaved *)SOMMalloc( sizeof( WrpWpSaved ) + size ) ) ) return( FALSE );
    if( !( s->cls = copy_string( cls ) ) ) { SOMFree( s ); return( FALSE ); }
    s->key = key;
    s->kind = kind;
    s->size = size;
    if( size ) memcpy( s->data, data, size );
    link = find_saved( somThis, cls, key );
    if( *link ) {
        s->next = ( *link )->next;
        SOMFree( ( *link )->cls );
        SOMFree( *link );
    } else {
        s->next = 0;
    }
    *link = s;
    return( TRUE );
}

/* What was saved under (cls, key) as kind; NULL when nothing was. */
static WrpWpSaved *saved( WPObject *somSelf, PSZ cls, ULONG key, ULONG kind )
{
    WrpWpSaved *s;
    if( !cls ) return( 0 );
    s = *find_saved( WPObjectGetData( somSelf ), cls, key );
    return( s && s->kind == kind ? s : 0 );
}

/* Copy a saved block to (value, *pcb): its size alone when value is NULL;
 * FALSE (and the size needed) when the buffer is too small. */
static BOOL restore( WrpWpSaved *s, void *value, PULONG pcb )
{
    if( !s || !pcb ) return( FALSE );
    if( value && *pcb < s->size ) { *pcb = s->size; return( FALSE ); }
    if( value ) memcpy( value, s->data, s->size );
    *pcb = s->size;
    return( TRUE );
}

SOM_Scope BOOL SOMLINK wpobj_wpSaveString( WPObject *somSelf, PSZ pszClass, ULONG ulKey, PSZ pszValue )
{
    return( pszValue && save( somSelf, pszClass, ulKey, SAVED_STRING, pszValue, strlen( pszValue ) + 1 ) );
}

SOM_Scope BOOL SOMLINK wpobj_wpRestoreString( WPObject *somSelf, PSZ pszClass, ULONG ulKey, PSZ pszValue, PULONG pcbValue )
{
    return( restore( saved( somSelf, pszClass, ulKey, SAVED_STRING ), pszValue, pcbValue ) );
}

SOM_Scope BOOL SOMLINK wpobj_wpSaveLong( WPObject *somSelf, PSZ pszClass, ULONG ulKey, ULONG ulValue )
{
    return( save( somSelf, pszClass, ulKey, SAVED_LONG, &ulValue, sizeof( ulValue ) ) );
}

SOM_Scope BOOL SOMLINK wpobj_wpRestoreLong( WPObject *somSelf, PSZ pszClass, ULONG ulKey, PULONG pulValue )
{
    ULONG cb = sizeof( ULONG );
    return( pulValue && restore( saved( somSelf, pszClass, ulKey, SAVED_LONG ), pulValue, &cb ) );
}

SOM_Scope BOOL SOMLINK wpobj_wpSaveData( WPObject *somSelf, PSZ pszClass, ULONG ulKey, PBYTE pValue, ULONG cbValue )
{
    return( save( somSelf, pszClass, ulKey, SAVED_DATA, pValue, cbValue ) );
}

SOM_Scope BOOL SOMLINK wpobj_wpRestoreData( WPObject *somSelf, PSZ pszClass, ULONG ulKey, PBYTE pValue, PULONG pcbValue )
{
    return( restore( saved( somSelf, pszClass, ulKey, SAVED_DATA ), pValue, pcbValue ) );
}

/* WPObject itself keeps nothing yet: a storage class saves its state. */
SOM_Scope BOOL SOMLINK wpobj_wpSaveState( WPObject *somSelf )
{
    (void)somSelf;
    return( TRUE );
}

SOM_Scope BOOL SOMLINK wpobj_wpRestoreState( WPObject *somSelf, ULONG ulReserved )
{
    (void)somSelf;
    (void)ulReserved;
    return( TRUE );
}

SOM_Scope BOOL SOMLINK wpobj_wpSaveImmediate( WPObject *somSelf )
{
    return( _wpSaveState( somSelf ) );
}

/* Saved at once: there is no deferred-save thread yet. */
SOM_Scope BOOL SOMLINK wpobj_wpSaveDeferred( WPObject *somSelf )
{
    return( _wpSaveImmediate( somSelf ) );
}

/* ---- WPObject: lifetime ---- */

SOM_Scope BOOL SOMLINK wpobj_wpFree( WPObject *somSelf )
{
    _somFree( somSelf );
    return( TRUE );
}

SOM_Scope ULONG SOMLINK wpobj_wpConfirmDelete( WPObject *somSelf, ULONG fConfirmations )
{
    (void)fConfirmations;
    return( ( _wpQueryStyle( somSelf ) & OBJSTYLE_NODELETE ) ? NO_DELETE : OK_DELETE );
}

SOM_Scope ULONG SOMLINK wpobj_wpDelete( WPObject *somSelf, ULONG fConfirmations )
{
    ULONG rc = _wpConfirmDelete( somSelf, fConfirmations );
    if( rc == OK_DELETE && !_wpFree( somSelf ) ) return( NO_DELETE );
    return( rc );
}

SOM_Scope BOOL SOMLINK wpobj_wpIsObjectInitialized( WPObject *somSelf )
{
    return( WPObjectGetData( somSelf )->initialized );
}

SOM_Scope void SOMLINK wpobj_wpLockObject( WPObject *somSelf )
{
    WPObjectGetData( somSelf )->locks++;
}

SOM_Scope BOOL SOMLINK wpobj_wpUnlockObject( WPObject *somSelf )
{
    WPObjectData *somThis = WPObjectGetData( somSelf );
    if( _locks == 0 ) return( FALSE );
    _locks--;
    return( TRUE );
}

SOM_Scope BOOL SOMLINK wpobj_wpIsLocked( WPObject *somSelf )
{
    return( WPObjectGetData( somSelf )->locks != 0 );
}

SOM_Scope WPFolder *SOMLINK wpobj_wpQueryFolder( WPObject *somSelf )
{
    return( WPObjectGetData( somSelf )->folder );
}

SOM_Scope void SOMLINK wpobj_somDefaultInit( WPObject *somSelf, somInitCtrl *ctrl )
{
    SOM_InitBegin( ctrl, _WPObject );
    _wpInitData( somSelf );
    SOM_InitEnd();
}

SOM_Scope void SOMLINK wpobj_somDestruct( WPObject *somSelf, octet doFree, somDestructCtrl *ctrl )
{
    SOM_DestructBegin( ctrl, doFree, _WPObject );
    _wpUnInitData( somSelf );
    SOM_DestructEnd( ctrl, doFree );
}

/* ---- M_WPObject ---- */

SOM_Scope WPObject *SOMLINK wpobjM_wpclsNew( M_WPObject *somSelf, PSZ pszTitle, PSZ pszSetupEnv,
                                             WPFolder *Folder, BOOL fLock )
{
    WPObject *obj;
    PSZ       title;
    /* No object of WPObject itself (the Reference). */
    if( (SOMClass *)somSelf == _WPObject ) return( 0 );
    if( !( obj = (WPObject *)_somNew( somSelf ) ) ) return( 0 );
    WPObjectGetData( obj )->folder = Folder;
    title = ( pszTitle && *pszTitle ) ? pszTitle : _wpclsQueryTitle( somSelf );
    if( title && !_wpSetTitle( obj, title ) ) { _wpFree( obj ); return( 0 ); }
    if( fLock ) _wpLockObject( obj );
    if( !_wpSetupOnce( obj, pszSetupEnv ? pszSetupEnv : (PSZ)"" ) ) { _wpFree( obj ); return( 0 ); }
    WPObjectGetData( obj )->initialized = TRUE;
    return( obj );
}

/* The class's name. */
SOM_Scope PSZ SOMLINK wpobjM_wpclsQueryTitle( M_WPObject *somSelf )
{
    return( _somGetName( somSelf ) );
}

SOM_Scope ULONG SOMLINK wpobjM_wpclsQueryStyle( M_WPObject *somSelf )
{
    (void)somSelf;
    return( 0 );
}

SOM_Scope ULONG SOMLINK wpobjM_wpclsQueryDefaultView( M_WPObject *somSelf )
{
    (void)somSelf;
    return( OPEN_SETTINGS );
}

SOM_Scope void SOMLINK wpobjM_wpclsInitData( M_WPObject *somSelf )
{
    (void)somSelf;
}

SOM_Scope void SOMLINK wpobjM_wpclsUnInitData( M_WPObject *somSelf )
{
    (void)somSelf;
}

SOM_Scope void SOMLINK wpobjM_somDefaultInit( M_WPObject *somSelf, somInitCtrl *ctrl )
{
    SOM_InitBegin( ctrl, _M_WPObject );
    _wpclsInitData( somSelf );
    SOM_InitEnd();
}
