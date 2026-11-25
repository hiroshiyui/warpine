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
 * Last, TypeCodes (ADR-0128): sizes and alignments against this
 * compiler's own, parameters as an any, the Bounds exception, copies,
 * constants and TypeCode_print's text.
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

/* TypeCode_print's text, caught through somSetOutChar. */
static char printed[256];

static int SOMLINK catch_char( char c )
{
    size_t n = strlen( printed );
    if( n + 1 < sizeof( printed ) ) { printed[n] = c; printed[n + 1] = '\0'; }
    return( 1 );
}

struct node { long count; struct node *next; };
struct mixed { char a; double b; short c; };
struct tagged { short _d; union { char a; double d; } _u; };

static void typecode_checks( void )
{
    Environment *ev = somGetGlobalEnvironment();
    TypeCode     node, mixed, tagged, color, copy, ptr;
    any          p;

    /* The Reference's example: nested calls, the outer one owns them. */
    node = TypeCodeNew( tk_struct, "node", "count", TypeCodeNew( tk_long ),
                        "next", TypeCodeNew( tk_pointer, TypeCodeNew( tk_self, "node" ) ), NULL );
    check( "TypeCodeNew: a self-referential struct",
           node && TypeCode_kind( node, ev ) == tk_struct && TypeCode_param_count( node, ev ) == 5 );
    check( "TypeCode_size / _alignment: struct node as this compiler lays it out",
           TypeCode_size( node, ev ) == (long)sizeof( struct node )
           && TypeCode_alignment( node, ev ) == (short)__alignof__( struct node ) );
    mixed = TypeCodeNew( tk_struct, "mixed", "a", TC_char, "b", TC_double, "c", TC_short, NULL );
    check( "TypeCode_size: char, double, short with padding",
           TypeCode_size( mixed, ev ) == (long)sizeof( struct mixed )
           && TypeCode_alignment( mixed, ev ) == (short)__alignof__( struct mixed ) );
    tagged = TypeCodeNew( tk_union, "tagged", TC_short,
                          TCREGULAR_CASE, 1L, "a", TC_char,
                          TCDEFAULT_CASE, 0L, "d", TC_double, NULL );
    check( "TypeCode_size: a union with its discriminator",
           TypeCode_param_count( tagged, ev ) == 8 && TypeCode_size( tagged, ev ) == (long)sizeof( struct tagged ) );
    check( "TypeCode_size: any and a sequence",
           TypeCode_size( TC_any, ev ) == (long)sizeof( any )
           && TypeCode_size( TypeCodeNew( tk_sequence, TC_long, 0L ), ev ) == (long)sizeof( SOMClassSequence ) );

    /* A parameter as an any: a string's address. */
    color = TypeCodeNew( tk_enum, "color", "red", "green", NULL );
    p = TypeCode_parameter( color, ev, 2 );
    check( "TypeCode_parameter: TC_string and the enumerator's name",
           ev->_major == NO_EXCEPTION && TypeCode_equal( p._type, ev, TC_string )
           && strcmp( *(string *)p._value, "green" ) == 0 );
    p = TypeCode_parameter( color, ev, 3 );
    check( "TypeCode_parameter: out of range raises TypeCode::Bounds",
           ev->_major == USER_EXCEPTION && strcmp( somExceptionId( ev ), ex_TypeCode_Bounds ) == 0
           && p._type == NULL );
    somExceptionFree( ev );

    /* A copy equals its original until its alignment changes, and
     * outlives it. */
    copy = TypeCode_copy( node, ev );
    check( "TypeCode_copy / _equal", copy && copy != node && TypeCode_equal( copy, ev, node ) );
    TypeCode_setAlignment( copy, ev, 8 );
    check( "TypeCode_setAlignment: the alignment counts in _equal",
           TypeCode_alignment( copy, ev ) == 8 && !TypeCode_equal( copy, ev, node ) );
    TypeCode_free( node, ev );
    check( "TypeCode_free: the copy lives on", TypeCode_size( copy, ev ) == (long)sizeof( struct node ) );

    /* Constants are shared and survive TypeCode_free. */
    TypeCode_free( TC_long, ev );
    check( "TC_ constants: shared, not freed", TC_long == TC_long && TypeCode_kind( TC_long, ev ) == tk_long );

    /* TypeCode_print writes the TypeCodeNew call that makes it. */
    ptr = TypeCodeNew( tk_pointer, TypeCodeNew( tk_string, 0L ) );
    somSetOutChar( catch_char );
    TypeCode_print( ptr, ev );
    somSetOutChar( 0 );
    check( "TypeCode_print: as a TypeCodeNew call",
           strcmp( printed, "TypeCodeNew (tk_pointer, TypeCodeNew (tk_string, 0))\n" ) == 0 );
    TypeCode_free( ptr, ev );
    TypeCode_free( copy, ev );
    TypeCode_free( mixed, ev );
    TypeCode_free( tagged, ev );
    TypeCode_free( color, ev );
}

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
        {
            SOMClassSequence parents = _somGetParents( Bottom );
            somOffsets offsets = __get_somInstanceDataOffsets( Bottom );
            SOMClassSequence none = _somGetParents( SOMObjectClassData.classObject );
            check( "somGetParents: left to right (a sequence by value)",
                   parents._length == 2 && parents._buffer[0] == Left && parents._buffer[1] == Right );
            check( "somGetParents: none for SOMObject", none._length == 0 );
            check( "_get_somInstanceDataOffsets: Bottom's own data last",
                   offsets._length >= 2 && offsets._buffer[offsets._length - 1].cls == Bottom
                   && offsets._buffer[offsets._length - 1].offset > 0 );
            check( "somGetRelatedClasses: no affinity group without a class file",
                   _somGetRelatedClasses( SOMClassMgrObject, Base ) == NULL );
            SOMFree( parents._buffer );
            SOMFree( offsets._buffer );
        }
        check( "somGetInitFunction", strcmp( _somGetInitFunction( SOMClassMgrObject ), "SOMInitModule" ) == 0 );
        check( "somMainProgram", SOM_MainProgram() == SOMClassMgrObject );
        SOM_Test( total > 0 );
        SOM_TestC( 0 );
        SOM_Assert( 1, 20012 );
        check( "SOM_Test / SOM_TestC / SOM_Assert: quiet at the default levels", 1 );
        typecode_checks();
        somEnvironmentEnd();
    }

    printf( "c_som_test: done %d/%d\n", passed, total );
    return( passed == total ? 0 : 1 );
}
