/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * somapi.h - the SOM runtime kernel's types, functions and runtime
 * variables for GCC and clang (ADR-0122)
 *
 * Written for Warpine from IBM's public SOM documentation (the System
 * Object Model Programming Reference and Guide); nothing is copied from
 * IBM's headers. som.h includes it.
 *
 * lx-link links an import as a call thunk, so SOM's data exports (the
 * root classes' ClassData, SOMClassMgrObject, the runtime variables) are
 * reached through WrpSomDataExport, which gives their addresses: each
 * name below is a macro for the variable itself, so it reads and assigns
 * as IBM's does (`SOMOutCharRoutine = my_routine;`).
 */
#ifndef WARPINE_SOMAPI_H
#define WARPINE_SOMAPI_H

#include <stddef.h>
#include <stdarg.h>

/* SOM functions and methods use the system calling convention: the
 * caller pops the arguments, as for APIENTRY. */
#define SOMLINK   __attribute__((__cdecl__))
#define SOMDLINK
#ifdef __cplusplus
#define SOMEXTERN extern "C"
#else
#define SOMEXTERN extern
#endif

typedef long           integer4;
typedef unsigned long  uinteger4;
typedef unsigned char  octet;
typedef unsigned char  boolean;
typedef char          *zString;
typedef char          *string;
typedef void          *somToken;
typedef unsigned long  somMToken;
typedef unsigned long  somDToken;
typedef char         **somId;
typedef void SOMLINK   somMethodProc( void );
typedef somMethodProc *somMethodPtr;

struct SOMAny_struct;
/* A method table: a header, then the procedures in the order of the
 * class's methods (ADR-0072). */
typedef struct somMethodTabStruct {
    struct SOMAny_struct *classObject;
    void                 *classInfo;
    char                 *className;
    long                  instanceSize;
    long                  dataAlignment;
    long                  mtabSize;         /* bytes, header included */
    long                  protectedDataOffset;
    somDToken             protectedDataToken;
    void                 *embeddedObjs;
    somMethodProc        *entries[1];
} somMethodTab;

/* A class's own table and its parents' (<Class>CClassData.parentMtab). */
typedef struct somMethodTabList {
    somMethodTab            *mtab;
    struct somMethodTabList *next;
} somMethodTabList, *somMethodTabs;

typedef struct somParentMtabStruct {
    somMethodTab  *mtab;
    somMethodTabs  next;
    struct SOMAny_struct *classObject;
    somMethodProc *somRenewNoInitNoZeroThunk;
    long           instanceSize;
    /* More words follow in IBM's layout; Warpine leaves them 0. */
} somParentMtabStruct, *somParentMtabStructPtr;

/* Every object begins with a pointer to its class's method table. */
typedef struct SOMAny_struct {
    somMethodTab *mtab;
} SOMAny;

#ifndef __cplusplus
typedef SOMAny SOMObject;
typedef SOMAny SOMClass;
typedef SOMAny SOMClassMgr;
#else
class SOMObject;
class SOMClass;
class SOMClassMgr;
#endif

/* sequence<SOMClass>: the parents somInitMIClass takes. */
typedef struct SOMClassSequence {
    unsigned long   _maximum;
    unsigned long   _length;
    SOMClass      **_buffer;
} SOMClassSequence;

/* A NULL-terminated array of class objects (somGetRelatedClasses). */
typedef SOMClass **SOMClassArray;

/* somOffsets: where each class's instance data lies in an instance of a
 * class (_get_somInstanceDataOffsets). */
typedef struct somOffsetInfo {
    SOMClass *cls;
    long      offset;
} somOffsetInfo;

typedef struct somOffsets {
    unsigned long   _maximum;
    unsigned long   _length;
    somOffsetInfo  *_buffer;
} somOffsets;

typedef struct Environment {
    long  _major;
    void *exception_id;
    void *params;
    long  reserved;
} Environment;

/* ---- Exceptions in an Environment (ADR-0122, stage S4) ----
 * A method declared with `raises` reports an exception by
 * somSetException( ev, USER_EXCEPTION, ex_<Name>, value ): value is the
 * exception's struct, from SOMMalloc, owned by the Environment until
 * somExceptionFree. The caller checks ev->_major. */
typedef enum exception_type { NO_EXCEPTION, USER_EXCEPTION, SYSTEM_EXCEPTION } exception_type;
SOMEXTERN void    SOMLINK somSetException( Environment *ev, exception_type major, zString exceptionName, void *params );
SOMEXTERN zString SOMLINK somExceptionId( Environment *ev );
SOMEXTERN void  * SOMLINK somExceptionValue( Environment *ev );
SOMEXTERN void    SOMLINK somExceptionFree( Environment *ev );
#define SOM_InitEnvironment( ev ) \
    ( (ev)->_major = NO_EXCEPTION, (ev)->exception_id = 0, (ev)->params = 0 )
#define SOM_UninitEnvironment( ev ) somExceptionFree( ev )

/* CORBA's any: a TypeCode and the value. A TypeCode's representation is
 * hidden (SOM Programming Reference): use the TypeCode functions below. */
typedef void *TypeCode;
typedef struct any {
    TypeCode  _type;
    void     *_value;
} any;

/* ---- Warpine's initializer and destructor protocol (ADR-0122) ----
 * Not IBM's binary layout, which is not published: a class built with
 * somidl's bindings initializes its ancestors through these. A table
 * lists the ancestors of the class being made, each once, parents'
 * ancestors first and the class itself last, with the somDefaultInit and
 * somDestruct each introduced or overrode (0 for inherited ones); a
 * control block flags the entries still to run. */
typedef struct somInitInfo {
    SOMClass      *cls;
    somMethodProc *defaultInit;   /* 0: the class inherits it */
    somMethodProc *destruct;      /* 0: the class inherits it */
    long           dataOffset;    /* the class's data in the instance */
} somInitInfo;

typedef struct somInitCtrl {
    octet       *mask;            /* 1: entry still to run */
    somInitInfo *info;
    long         infoSize;        /* sizeof(somInitInfo) */
    long         count;
} somInitCtrl;
typedef somInitCtrl somDestructCtrl;
typedef somInitCtrl somAssignCtrl;

/* A control block for an instance of `cls` (every ancestor flagged, not
 * `cls`), from SOMMalloc; 0 if `cls` is not a ready class. */
SOMEXTERN somInitCtrl * SOMLINK WrpSomInitCtrlNew( SOMClass *cls );
SOMEXTERN void          SOMLINK WrpSomInitCtrlFree( somInitCtrl *ctrl );
/* Run the whole protocol for `obj`'s class: every own initializer in
 * order, or destructor last first (then the memory, for doFree). An
 * initializer or destructor the class inherited hands over to these. */
SOMEXTERN void          SOMLINK WrpSomInitWalk( SOMObject *obj );
SOMEXTERN void          SOMLINK WrpSomDestructWalk( SOMObject *obj, octet doFree );
/* The ancestors class `cls`'s initializers visit, in that order, instead
 * of its parents (`directinitclasses`; `n` somIds): 0 for success. */
SOMEXTERN long          SOMLINK WrpSomSetDirectInitClasses( SOMClass *cls, long n, somId *names );


/* What somBuildClass takes (layout version 1, as somidl's bindings fill
 * it): a class's static methods and overrides, its parents and metaclass
 * by name (ADR-0073). */
typedef struct somStaticMethod_t {
    somMToken     *classData;       /* where its token goes */
    somId         *methodId;
    somId         *methodDescriptor;
    somMethodProc *method;
    somMethodProc *redispatchStub;
    somMethodProc *applyStub;
} somStaticMethod_t;
typedef struct somOverrideMethod_t {
    somId         *methodId;
    somMethodProc *method;
} somOverrideMethod_t;
typedef struct somStaticClassInfo {
    unsigned long        layoutVersion;
    unsigned long        numStaticMethods;
    unsigned long        numStaticOverrides;
    unsigned long        numNonInternalData;
    unsigned long        numProcMethods;
    unsigned long        numVarargsFuncs;
    unsigned long        majorVersion;
    unsigned long        minorVersion;
    unsigned long        instanceDataSize;
    unsigned long        maxMethods;
    unsigned long        numParents;
    somId                classId;
    somId                explicitMetaId;
    long                 implicitParentMeta;
    somId               *parents;
    void                *cds;            /* the class's ClassData */
    void                *ccds;           /* its CClassData */
    somStaticMethod_t   *smt;
    somOverrideMethod_t *omt;
    char                *nitReferenceBase;
    void                *nit;
    void                *pmt;
    void                *vft;
    void                *cif;
} somStaticClassInfo;

/* ---- Functions ---- */
SOMEXTERN SOMClassMgr   * SOMLINK somEnvironmentNew( void );
SOMEXTERN Environment   * SOMLINK somGetGlobalEnvironment( void );
SOMEXTERN somMethodProc * SOMLINK somResolve( SOMObject *obj, somMToken method );
SOMEXTERN somMethodProc * SOMLINK somResolveByName( SOMObject *obj, char *name );
SOMEXTERN somMethodProc * SOMLINK somClassResolve( SOMClass *cls, somMToken method );
SOMEXTERN somMethodProc * SOMLINK somParentResolve( somMethodTabs parentMtabs, somMToken method );
SOMEXTERN somMethodProc * SOMLINK somParentNumResolve( somMethodTabs parentMtabs, int parentNum, somMToken method );
SOMEXTERN void          * SOMLINK somDataResolve( SOMObject *obj, somDToken data );
SOMEXTERN void          * SOMLINK somDataResolveChk( SOMObject *obj, somDToken data );
SOMEXTERN int             SOMLINK somIsObj( somToken p );
SOMEXTERN somId           SOMLINK somIdFromString( zString s );
SOMEXTERN zString         SOMLINK somStringFromId( somId id );
SOMEXTERN int             SOMLINK somCompareIds( somId a, somId b );
SOMEXTERN somId           SOMLINK somCheckId( somId id );
SOMEXTERN int             SOMLINK somRegisterId( somId id );
SOMEXTERN unsigned long   SOMLINK somUniqueKey( somId id );
SOMEXTERN unsigned long   SOMLINK somTotalRegIds( void );
SOMEXTERN void            SOMLINK somSetExpectedIds( unsigned long numIds );
SOMEXTERN void            SOMLINK somBeginPersistentIds( void );
SOMEXTERN void            SOMLINK somEndPersistentIds( void );
SOMEXTERN int             SOMLINK somPrintf( zString fmt, ... );
SOMEXTERN int             SOMLINK somVprintf( zString fmt, void *ap );
SOMEXTERN int             SOMLINK somLPrintf( int level, zString fmt, ... );
SOMEXTERN void            SOMLINK somPrefixLevel( long level );
SOMEXTERN SOMObject     * SOMLINK somTestCls( SOMObject *obj, SOMClass *cls, zString fileName, int lineNum );
SOMEXTERN void            SOMLINK somTest( int condition, int severity, zString fileName, int lineNum, zString msg );
SOMEXTERN void            SOMLINK somAssert( int condition, int ecode, zString fileName, int lineNum, zString msg );
SOMEXTERN SOMClass      * SOMLINK somBuildClass( long inheritVars, somStaticClassInfo *sci, long majorVersion, long minorVersion );
SOMEXTERN SOMClass      * SOMLINK SOMObjectNewClass( integer4 majorVersion, integer4 minorVersion );
SOMEXTERN SOMClass      * SOMLINK SOMClassNewClass( integer4 majorVersion, integer4 minorVersion );
SOMEXTERN SOMClass      * SOMLINK SOMClassMgrNewClass( integer4 majorVersion, integer4 minorVersion );
/* Warpine's: the metaclass of a new class that asks for `requested` (0:
 * none) and has parent `parent`, a derived one when they clash. */
SOMEXTERN SOMClass      * SOMLINK WrpSomMetaclassFor( SOMClass *requested, SOMClass *parent );
/* Warpine's: the address of SOM data export `ordinal` (1-19), 0 for
 * another ordinal. */
SOMEXTERN void          * SOMLINK WrpSomDataExport( unsigned long ordinal );

/* ---- Runtime variables (SOM data exports, by their ordinals) ---- */
typedef void * SOMLINK somTD_SOMMalloc( size_t size );
typedef void * SOMLINK somTD_SOMCalloc( size_t count, size_t size );
typedef void * SOMLINK somTD_SOMRealloc( void *p, size_t size );
typedef void   SOMLINK somTD_SOMFree( void *p );
typedef int    SOMLINK somTD_SOMOutCharRoutine( char c );
typedef int    SOMLINK somTD_SOMLoadModule( zString className, zString fileName, zString functionName,
                                            long majorVersion, long minorVersion, somToken *modHandle );
typedef int    SOMLINK somTD_SOMDeleteModule( somToken modHandle );
typedef void   SOMLINK somTD_SOMError( int code, zString fileName, int lineNum );
typedef zString SOMLINK somTD_SOMClassInitFuncName( void );

#define WRP_SOM_DATA( type, ordinal ) (*(type *)WrpSomDataExport( ordinal ))
#define SOMCalloc            WRP_SOM_DATA( somTD_SOMCalloc *, 1 )
#define SOMClassInitFuncName WRP_SOM_DATA( somTD_SOMClassInitFuncName *, 4 )
#define SOMClassMgrObject    WRP_SOM_DATA( SOMClassMgr *, 7 )
#define SOMDeleteModule      WRP_SOM_DATA( somTD_SOMDeleteModule *, 8 )
#define SOMError             WRP_SOM_DATA( somTD_SOMError *, 9 )
#define SOMFree              WRP_SOM_DATA( somTD_SOMFree *, 10 )
#define SOMLoadModule        WRP_SOM_DATA( somTD_SOMLoadModule *, 11 )
#define SOMMalloc            WRP_SOM_DATA( somTD_SOMMalloc *, 12 )
#define SOMOutCharRoutine    WRP_SOM_DATA( somTD_SOMOutCharRoutine *, 15 )
#define SOMRealloc           WRP_SOM_DATA( somTD_SOMRealloc *, 16 )
#define SOM_AssertLevel      WRP_SOM_DATA( long, 17 )
#define SOM_TraceLevel       WRP_SOM_DATA( long, 18 )
#define SOM_WarnLevel        WRP_SOM_DATA( long, 19 )

/* somSetOutChar( 0 ) puts the default routine back (Warpine's). */
SOMEXTERN void SOMLINK somSetOutChar( somTD_SOMOutCharRoutine *routine );

/* A SOM error code's last digit is its severity: 9 fatal, 2 a warning,
 * 1 to ignore (SOM Programming Reference). */
#define SOM_Ignore  1
#define SOM_Warn    2
#define SOM_Fatal   9
#define SOM_Error( code ) SOMError( (code), __FILE__, __LINE__ )

/* The runtime for a program, and its end (nothing to release: the
 * runtime lives as long as the process). */
SOMEXTERN SOMClassMgr * SOMLINK somMainProgram( void );
SOMEXTERN void          SOMLINK somEnvironmentEnd( void );
#define SOM_MainProgram() somMainProgram()

/* Tests and assertions (SOM Programming Reference): SOM_Test is fatal
 * when it fails, SOM_TestC and SOM_Expect warn (above SOM_WarnLevel 0);
 * a passed one is noted above SOM_AssertLevel 0. */
#define SOM_Test( b )        somTest( (b), SOM_Fatal, __FILE__, __LINE__, #b )
#define SOM_TestC( b )       somTest( (b), SOM_Warn, __FILE__, __LINE__, #b )
#define SOM_Expect( b )      somTest( (b), SOM_Warn, __FILE__, __LINE__, #b )
#define SOM_Assert( b, ec )  somAssert( (b), (ec), __FILE__, __LINE__, #b )
#define SOM_WarnMsg( m ) \
    ( SOM_WarnLevel > 0 ? somPrintf( "\"%s\": %d:\tWarning: %s\n", __FILE__, __LINE__, (m) ) : 0 )

/* What somGetMethodData gives: the method's id, type (0: static),
 * descriptor (none kept: 0), token, procedure and shared data (0). */
typedef struct somMethodData {
    somId          id;
    long           type;
    somId          descriptor;
    somMToken      mToken;
    somMethodPtr   method;
    void          *shared;
} somMethodData;

/* ---- Dynamic dispatch (ADR-0126) ---- */

/* TCKind: the kinds of TypeCodes, in the order the SOM Programming
 * Reference lists them (TypeCode_kind); somVaBuf_add takes the scalar
 * ones. The values follow from C's numbering (unverified against IBM's
 * headers). */
typedef enum TCKind {
    tk_null, tk_void, tk_short, tk_long, tk_ushort, tk_ulong, tk_float,
    tk_double, tk_boolean, tk_char, tk_octet, tk_any, tk_TypeCode,
    tk_Principal, tk_objref, tk_struct, tk_union, tk_enum, tk_string,
    tk_sequence, tk_array, tk_pointer, tk_self, tk_foreign
} TCKind;

/* ---- TypeCodes (ADR-0128) ----
 * TypeCodeNew's arguments depend on the kind:
 *   TypeCodeNew( tk_objref, interfaceId )
 *   TypeCodeNew( tk_string, maxLength )            0: unbounded
 *   TypeCodeNew( tk_sequence, elementTC, maxLength )
 *   TypeCodeNew( tk_array, elementTC, length )
 *   TypeCodeNew( tk_pointer, referencedTC )
 *   TypeCodeNew( tk_self, structOrUnionName )
 *   TypeCodeNew( tk_foreign, typeName, implementationContext, size )
 *   TypeCodeNew( tk_struct, name, memberName, memberTC, ..., NULL )
 *   TypeCodeNew( tk_union, name, switchTC,
 *                TCREGULAR_CASE, label, memberName, memberTC,
 *                TCDEFAULT_CASE, 0, memberName, memberTC, ..., NULL )
 *   TypeCodeNew( tk_enum, name, enumeratorName, ..., NULL )
 *   TypeCodeNew( any other kind )
 * The new TypeCode copies the strings and takes the TypeCodes it is given
 * (one given twice, or held by another TypeCode, is copied); free it with
 * TypeCode_free, which frees those too. Sizes are those of this SDK's C
 * types. TypeCode_parameter raises ex_TypeCode_Bounds for an index out of
 * range. The TC_ constants are shared: freeing one does nothing. */
#define TCREGULAR_CASE 1
#define TCDEFAULT_CASE 2
#define ex_TypeCode_Bounds "::TypeCode::Bounds"
SOMEXTERN TypeCode SOMLINK TypeCodeNew( TCKind tag, ... );
SOMEXTERN short    SOMLINK TypeCode_alignment( TypeCode tc, Environment *ev );
SOMEXTERN TypeCode SOMLINK TypeCode_copy( TypeCode tc, Environment *ev );
SOMEXTERN boolean  SOMLINK TypeCode_equal( TypeCode tc, Environment *ev, TypeCode tc2 );
SOMEXTERN void     SOMLINK TypeCode_free( TypeCode tc, Environment *ev );
SOMEXTERN TCKind   SOMLINK TypeCode_kind( TypeCode tc, Environment *ev );
SOMEXTERN long     SOMLINK TypeCode_param_count( TypeCode tc, Environment *ev );
SOMEXTERN void     SOMLINK TypeCode_print( TypeCode tc, Environment *ev );
SOMEXTERN void     SOMLINK TypeCode_setAlignment( TypeCode tc, Environment *ev, short alignment );
SOMEXTERN long     SOMLINK TypeCode_size( TypeCode tc, Environment *ev );
/* Warpine's: TypeCode_parameter with its result stored at result, so no
 * compiler's convention for returning a struct is involved. */
SOMEXTERN any    * SOMLINK WrpTypeCode_parameter( any *result, TypeCode tc, Environment *ev, long index );
static __inline__ any TypeCode_parameter( TypeCode tc, Environment *ev, long index )
{
    any a;
    WrpTypeCode_parameter( &a, tc, ev, index );
    return a;
}
/* Warpine's: the TC_ constant of a kind (the basic kinds, tk_objref for
 * TC_Object, tk_string for TC_string), NULL for another. */
SOMEXTERN TypeCode SOMLINK WrpTypeCodeConstant( TCKind kind );
#define TC_null      WrpTypeCodeConstant( tk_null )
#define TC_void      WrpTypeCodeConstant( tk_void )
#define TC_short     WrpTypeCodeConstant( tk_short )
#define TC_long      WrpTypeCodeConstant( tk_long )
#define TC_ushort    WrpTypeCodeConstant( tk_ushort )
#define TC_ulong     WrpTypeCodeConstant( tk_ulong )
#define TC_float     WrpTypeCodeConstant( tk_float )
#define TC_double    WrpTypeCodeConstant( tk_double )
#define TC_boolean   WrpTypeCodeConstant( tk_boolean )
#define TC_char      WrpTypeCodeConstant( tk_char )
#define TC_octet     WrpTypeCodeConstant( tk_octet )
#define TC_any       WrpTypeCodeConstant( tk_any )
#define TC_TypeCode  WrpTypeCodeConstant( tk_TypeCode )
#define TC_Principal WrpTypeCodeConstant( tk_Principal )
#define TC_Object    WrpTypeCodeConstant( tk_objref )
#define TC_string    WrpTypeCodeConstant( tk_string )

/* A buffer of arguments for a va_list (opaque). */
typedef void *somVaBuf;

/* Warpine's apply stubs: somidl writes one for each method of a class;
 * it reads the method's arguments (widened, as in a va_list) from args,
 * calls method with them and stores the result at retVal (when not NULL).
 * A class registers it with somAddStaticMethod's applyStub argument as a
 * somApplyInfo; anything else there is not used, and somApply of such a
 * method returns FALSE. */
typedef void SOMLINK somApplyStub( void *retVal, somMethodProc *method, char *args );
#define SOM_APPLY_INFO_MAGIC 0x53504157UL   /* "WAPS" */
typedef struct somApplyInfo {
    unsigned long magic;
    somApplyStub *stub;
} somApplyInfo;
/* The bytes a value of type t takes in a va_list (widened). */
#define SOM_VA_SIZE( t ) ( ( sizeof( t ) + 3 ) & ~3 )

/* somApply: calls the method mdPtr describes on objPtr with the arguments
 * of args (objPtr first), its result to retVal; FALSE when the method has
 * no apply stub. */
SOMEXTERN boolean       SOMLINK somApply( SOMObject *objPtr, somToken *retVal, somMethodData *mdPtr, va_list args );
SOMEXTERN somVaBuf      SOMLINK somVaBuf_create( char *vb, int size );
SOMEXTERN long          SOMLINK somVaBuf_add( somVaBuf vb, char *arg, int type );
SOMEXTERN void          SOMLINK somVaBuf_get_valist( somVaBuf vb, va_list *ap );
SOMEXTERN void          SOMLINK somVaBuf_destroy( somVaBuf vb );
SOMEXTERN unsigned long SOMLINK somvalistGetTarget( va_list ap );
SOMEXTERN void          SOMLINK somvalistSetTarget( va_list ap, unsigned long val );

#endif
