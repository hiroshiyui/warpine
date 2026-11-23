/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * c_som_test.c - SOM from GCC or clang with the SDK's som.h (ADR-0122)
 *
 * The SDK's headers reach the kernel's data exports through
 * WrpSomDataExport (SOMClassMgrObject, the root classes' ClassData, the
 * runtime variables), and its initializer protocol runs each class's own
 * somDefaultInit once, ancestors first, and its somDestruct in the
 * reverse order:
 *   - a diamond (Base; Left and Right from it; Bottom from both), every
 *     class with its own initializer and destructor;
 *   - Heir, which has none of its own: Left's, inherited, hands the walk
 *     to the root;
 *   - Combo, from Plain and Mixin, gets Mixin's by the merge rule: the
 *     root's walk still gives SOMObject's somInit its turn.
 * Then the entries the SOM Programming Reference documents: somRenew in
 * the caller's memory, somGetMethodIndex / somGetMethodData /
 * somGetMemberToken, somCastObj / somResetObj, somGetInitFunction,
 * somMainProgram and the SOM_Test / SOM_Assert macros.
 * Each check is written as "c_som_test: <name>: ok" or "... FAIL", then
 * "c_som_test: done <passed>/<total>".
 */
#include <som.h>
#include <stdio.h>
#include <string.h>

static int  passed, total;
static char trail[64];

static void check( const char *name, int ok )
{
    total++;
    if( ok ) passed++;
    printf( "c_som_test: %s: %s\n", name, ok ? "ok" : "FAIL" );
}

static void step( char c )
{
    size_t n = strlen( trail );
    if( n + 1 < sizeof( trail ) ) { trail[n] = c; trail[n + 1] = '\0'; }
}

static SOMClass *Base, *Left, *Right, *Bottom, *Heir, *Plain, *Mixin, *Combo;

/* Each class's own initializer and destructor: note the class, around the
 * protocol's calls to its ancestors. */
#define CLASS_INIT( fn, cls, letter ) \
    static void SOMLINK fn##Init( SOMObject *somSelf, somInitCtrl *ctrl ) \
    { \
        SOM_InitBegin( ctrl, cls ); \
        step( letter ); \
        SOM_InitEnd(); \
    } \
    static void SOMLINK fn##Destruct( SOMObject *somSelf, octet doFree, somDestructCtrl *ctrl ) \
    { \
        SOM_DestructBegin( ctrl, doFree, cls ); \
        step( (char)( letter + 'a' - 'A' ) ); \
        SOM_DestructEnd( ctrl, doFree ); \
    }

CLASS_INIT( base, Base, 'B' )
CLASS_INIT( left, Left, 'L' )
CLASS_INIT( right, Right, 'R' )
CLASS_INIT( bottom, Bottom, 'M' )
CLASS_INIT( mixin, Mixin, 'X' )

/* A class of SOMClass named `name`, with `n` parents, and its own
 * initializer and destructor when given. */
static SOMClass *make( const char *name, SOMClass **parents, int n,
                       somMethodProc *init, somMethodProc *destruct )
{
    SOMClass        *cls = _somNew( SOMClassClassData.classObject );
    SOMClassSequence seq;
    seq._maximum = seq._length = n;
    seq._buffer = parents;
    _somInitMIClass( cls, 0, (zString)name, seq, 4, 4, 0, 1, 0 );
    if( init ) _somOverrideSMethod( cls, somIdFromString( "somDefaultInit" ), init );
    if( destruct ) _somOverrideSMethod( cls, somIdFromString( "somDestruct" ), destruct );
    _somClassReady( cls );
    return( cls );
}

static const char *run( SOMClass *cls )
{
    SOMObject *obj;
    trail[0] = '\0';
    obj = _somNew( cls );
    step( '|' );
    _somFree( obj );
    return( trail );
}

#define ONE( p ) ( one[0] = (p), one )
#define TWO( p, q ) ( two[0] = (p), two[1] = (q), two )

int main( void )
{
    SOMClass *one[1], *two[2], *object;

    /* The data exports, through WrpSomDataExport. */
    check( "somEnvironmentNew is SOMClassMgrObject", somEnvironmentNew() == SOMClassMgrObject );
    object = SOMObjectClassData.classObject;
    check( "SOMObjectClassData", object != 0 && strcmp( _somGetName( object ), "SOMObject" ) == 0 );
    check( "SOMClassClassData", strcmp( _somGetClassName( SOMClassClassData.classObject ), "SOMClass" ) == 0 );
    check( "SOM_TraceLevel", SOM_TraceLevel == 0 );

    Base   = make( "Base", ONE( object ), 1, (somMethodProc *)baseInit, (somMethodProc *)baseDestruct );
    Left   = make( "Left", ONE( Base ), 1, (somMethodProc *)leftInit, (somMethodProc *)leftDestruct );
    Right  = make( "Right", ONE( Base ), 1, (somMethodProc *)rightInit, (somMethodProc *)rightDestruct );
    Bottom = make( "Bottom", TWO( Left, Right ), 2, (somMethodProc *)bottomInit, (somMethodProc *)bottomDestruct );
    Heir   = make( "Heir", ONE( Left ), 1, 0, 0 );
    Plain  = make( "Plain", ONE( object ), 1, 0, 0 );
    Mixin  = make( "Mixin", ONE( object ), 1, (somMethodProc *)mixinInit, (somMethodProc *)mixinDestruct );
    Combo  = make( "Combo", TWO( Plain, Mixin ), 2, 0, 0 );

    check( "Base: its own, then its own destructor", strcmp( run( Base ), "B|b" ) == 0 );
    check( "Left: Base first, destroyed last", strcmp( run( Left ), "BL|lb" ) == 0 );
    check( "the diamond: each class once, ancestors first", strcmp( run( Bottom ), "BLRM|mrlb" ) == 0 );
    check( "Heir: Left's inherited one hands over to the root", strcmp( run( Heir ), "BL|lb" ) == 0 );
    check( "Combo: Mixin's by the merge rule, the root walks", strcmp( run( Combo ), "X|x" ) == 0 );
    check( "Plain: nothing of its own, nothing to run", strcmp( run( Plain ), "|" ) == 0 );

    /* The Reference's other entries. */
    {
        static long   buffer[4];
        SOMObject    *made, *cast;
        somMethodData md;
        somId         id = somIdFromString( "somInit" );
        long          index;

        trail[0] = '\0';
        made = _somRenew( Base, buffer );
        check( "somRenew: in the caller's memory, initialized",
               made == (SOMObject *)buffer && strcmp( trail, "B" ) == 0 && somIsObj( made ) );
        check( "somRenewNoInit: no initializer",
               _somRenewNoInit( Base, buffer ) == (SOMObject *)buffer && strcmp( trail, "B" ) == 0 );
        index = _somGetMethodIndex( Base, id );
        check( "somGetMethodIndex / somGetNthMethodInfo",
               index >= 0 && strcmp( somStringFromId( _somGetNthMethodInfo( Base, index, 0 ) ), "somInit" ) == 0 );
        check( "somGetMethodData: the token and procedure",
               _somGetMethodData( Base, id, &md ) && md.mToken == SOMObjectClassData.somInit
               && md.method == somResolve( made, SOMObjectClassData.somInit ) );
        check( "somGetMemberToken: Base's first word",
               somDataResolve( made, _somGetMemberToken( Base, 0, _somGetInstanceToken( Base ) ) )
               == somDataResolve( made, _somGetInstanceToken( Base ) ) );
        cast = _somNew( Bottom );
        check( "somCastObj: the object answers as Left",
               _somCastObj( cast, Left ) && _somGetClass( cast ) == Left );
        check( "somResetObj: as Bottom again", _somResetObj( cast ) && _somGetClass( cast ) == Bottom );
        _somFree( cast );
        check( "somGetInitFunction", strcmp( _somGetInitFunction( SOMClassMgrObject ), "SOMInitModule" ) == 0 );
        check( "somMainProgram", SOM_MainProgram() == SOMClassMgrObject );
        SOM_Test( total > 0 );
        SOM_TestC( 0 );
        SOM_Assert( 1, 20012 );
        check( "SOM_Test / SOM_TestC / SOM_Assert: quiet at the default levels", 1 );
        somEnvironmentEnd();
    }

    printf( "c_som_test: done %d/%d\n", passed, total );
    return( passed == total ? 0 : 1 );
}
