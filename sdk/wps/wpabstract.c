/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * wpabstract.c - WPAbstract and M_WPAbstract (ADR-0129): Workplace
 * objects kept in the user profile, in Warpine's own record format
 * (wpsint.h). The code is Warpine's own.
 */
#define INCL_WINWORKPLACE
#define INCL_WINSHELLDATA
#include <os2.h>
#include <stdio.h>
#include <string.h>
#include "wpabstract.ih"
#include "wpsint.h"

/* How many taken handles the allocator steps over before it gives up. */
#define MAX_HANDLE_PROBES 4096

static void handle_key( HOBJECT h, char key[9] )
{
    sprintf( key, "%08lX", (unsigned long)h );
}

static BOOL stored( HOBJECT h )
{
    char  key[9];
    ULONG cb = 0;
    handle_key( h, key );
    return( PrfQueryProfileSize( HINI_USERPROFILE, WRP_WPS_APP_OBJECTS, key, &cb ) && cb > 0 );
}

/* The handle an object ID names in the user profile; NULLHANDLE when it
 * names none that is still stored. */
static HOBJECT id_handle( PSZ id )
{
    HOBJECT h = NULLHANDLE;
    ULONG   cb = sizeof( h );
    if( !id || !PrfQueryProfileData( HINI_USERPROFILE, WRP_WPS_APP_IDS, id, &h, &cb ) || cb != sizeof( h ) ) return( NULLHANDLE );
    return( stored( h ) ? h : NULLHANDLE );
}

/* A handle no stored object has. */
static HOBJECT new_handle( void )
{
    HOBJECT next = WRP_WPS_FIRST_HANDLE;
    ULONG   cb = sizeof( next ), i;
    if( !PrfQueryProfileData( HINI_USERPROFILE, WRP_WPS_APP_HANDLES, WRP_WPS_KEY_NEXT, &next, &cb )
        || cb != sizeof( next ) || next < WRP_WPS_FIRST_HANDLE )
        next = WRP_WPS_FIRST_HANDLE;
    for( i = 0; i < MAX_HANDLE_PROBES && stored( next ); i++ ) next++;
    if( i == MAX_HANDLE_PROBES ) return( NULLHANDLE );
    {
        HOBJECT after = next + 1;
        if( !PrfWriteProfileData( HINI_USERPROFILE, WRP_WPS_APP_HANDLES, WRP_WPS_KEY_NEXT, &after, sizeof( after ) ) )
            return( NULLHANDLE );
    }
    return( next );
}

static char *copy_string( const char *s )
{
    size_t n = strlen( s ) + 1;
    char  *p = (char *)SOMMalloc( n );
    if( p ) memcpy( p, s, n );
    return( p );
}

/* ---- WPAbstract ---- */

SOM_Scope void SOMLINK wpab_wpUnInitData( WPAbstract *somSelf )
{
    WPAbstractData *somThis = WPAbstractGetData( somSelf );
    if( _storedID ) SOMFree( _storedID );
    _storedID = 0;
    WPAbstract_parent_WPObject_wpUnInitData( somSelf );
}

/* The object's record written: its class, then its saved values (after
 * wpSaveState), under its handle (given now when it has none); its
 * object ID mapped to the handle. */
SOM_Scope BOOL SOMLINK wpab_wpSaveImmediate( WPAbstract *somSelf )
{
    WPAbstractData *somThis = WPAbstractGetData( somSelf );
    const char     *cls;
    PSZ             id;
    ULONG           name, part, size;
    unsigned char  *rec;
    char            key[9];
    BOOL            ok;
    if( !WPAbstract_parent_WPObject_wpSaveImmediate( somSelf ) ) return( FALSE );
    if( !_handle ) {
        HOBJECT h = new_handle();
        if( !h || !wrp_wps_register( h, somSelf ) ) return( FALSE );
        _handle = h;
    }
    cls = _somGetClassName( somSelf );
    name = (ULONG)strlen( cls );
    part = wrp_wps_store_write( somSelf, 0, 0 );
    size = 12 + name + part;
    if( size > WRP_WPS_MAX_RECORD || !( rec = (unsigned char *)SOMMalloc( size ) ) ) return( FALSE );
    {
        ULONG head[3];
        head[0] = WRP_WPS_RECORD_MAGIC;
        head[1] = WRP_WPS_RECORD_VERSION;
        head[2] = name;
        memcpy( rec, head, sizeof( head ) );
    }
    memcpy( rec + 12, cls, name );
    wrp_wps_store_write( somSelf, rec + 12 + name, part );
    handle_key( _handle, key );
    ok = PrfWriteProfileData( HINI_USERPROFILE, WRP_WPS_APP_OBJECTS, key, rec, size );
    SOMFree( rec );
    if( !ok ) return( FALSE );
    /* The object ID: an old one let go, the current one mapped. */
    id = _wpQueryObjectID( somSelf );
    if( _storedID && ( !id || strcmp( id, _storedID ) != 0 ) ) {
        if( id_handle( _storedID ) == _handle ) PrfWriteProfileData( HINI_USERPROFILE, WRP_WPS_APP_IDS, _storedID, 0, 0 );
        SOMFree( _storedID );
        _storedID = 0;
    }
    if( id && !_storedID ) {
        if( !PrfWriteProfileData( HINI_USERPROFILE, WRP_WPS_APP_IDS, id, &_handle, sizeof( _handle ) ) ) return( FALSE );
        _storedID = copy_string( id );
    }
    return( TRUE );
}

/* The persistent form goes first (the Reference), then the object. */
SOM_Scope BOOL SOMLINK wpab_wpFree( WPAbstract *somSelf )
{
    WPAbstractData *somThis = WPAbstractGetData( somSelf );
    if( _handle ) {
        char key[9];
        handle_key( _handle, key );
        PrfWriteProfileData( HINI_USERPROFILE, WRP_WPS_APP_OBJECTS, key, 0, 0 );
        if( _storedID && id_handle( _storedID ) == NULLHANDLE )
            PrfWriteProfileData( HINI_USERPROFILE, WRP_WPS_APP_IDS, _storedID, 0, 0 );
    }
    return( WPAbstract_parent_WPObject_wpFree( somSelf ) );
}

SOM_Scope HOBJECT SOMLINK wpab_wpQueryHandle( WPAbstract *somSelf )
{
    return( WPAbstractGetData( somSelf )->handle );
}

/* An object ID names one object: one another stored object has is
 * refused. */
SOM_Scope BOOL SOMLINK wpab_wpSetObjectID( WPAbstract *somSelf, PSZ pszObjectID )
{
    HOBJECT other = id_handle( pszObjectID );
    if( other && other != WPAbstractGetData( somSelf )->handle ) return( FALSE );
    return( WPAbstract_parent_WPObject_wpSetObjectID( somSelf, pszObjectID ) );
}

/* ---- M_WPAbstract ---- */

/* A new object is stored at once. */
SOM_Scope WPObject *SOMLINK wpabM_wpclsNew( M_WPAbstract *somSelf, PSZ pszTitle, PSZ pszSetupEnv,
                                            WPFolder *Folder, BOOL fLock )
{
    WPObject *obj = M_WPAbstract_parent_M_WPObject_wpclsNew( somSelf, pszTitle, pszSetupEnv, Folder, fLock );
    if( obj && !_wpSaveImmediate( obj ) ) {
        _wpFree( obj );
        return( 0 );
    }
    return( obj );
}

/* ---- Awakening ---- */

WPObject *wrp_wps_awaken( HOBJECT h )
{
    char           key[9], name[WP_MAX_TITLE];
    ULONG          size = 0, head[3];
    unsigned char *rec;
    SOMClass      *cls;
    WPObject      *obj = 0;
    handle_key( h, key );
    if( !PrfQueryProfileSize( HINI_USERPROFILE, WRP_WPS_APP_OBJECTS, key, &size ) || size < 12 || size > WRP_WPS_MAX_RECORD )
        return( 0 );
    if( !( rec = (unsigned char *)SOMMalloc( size ) ) ) return( 0 );
    if( !PrfQueryProfileData( HINI_USERPROFILE, WRP_WPS_APP_OBJECTS, key, rec, &size ) || size < 12 ) goto done;
    memcpy( head, rec, sizeof( head ) );
    if( head[0] != WRP_WPS_RECORD_MAGIC || head[1] != WRP_WPS_RECORD_VERSION || head[2] >= sizeof( name ) || head[2] > size - 12 )
        goto done;
    memcpy( name, rec + 12, head[2] );
    name[head[2]] = '\0';
    /* Only a registered class that stores its objects here. */
    cls = _somFindClass( SOMClassMgrObject, somIdFromString( name ), 0, 0 );
    if( !cls || !_somDescendedFrom( cls, _WPAbstract ) ) goto done;
    if( !( obj = (WPObject *)_somNew( cls ) ) ) goto done;
    WPAbstractGetData( obj )->handle = h;
    if( !wrp_wps_register( h, obj ) || !wrp_wps_store_read( obj, rec + 12 + head[2], size - 12 - head[2] )
        || !_wpRestoreState( obj, 0 ) ) {
        WPAbstractGetData( obj )->handle = NULLHANDLE;
        _somFree( obj );
        obj = 0;
        goto done;
    }
    WPAbstractGetData( (WPAbstract *)obj )->storedID = _wpQueryObjectID( obj ) ? copy_string( _wpQueryObjectID( obj ) ) : 0;
    wrp_wps_set_initialized( obj );
done:
    SOMFree( rec );
    return( obj );
}
