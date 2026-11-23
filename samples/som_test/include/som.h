/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * som.h - Warpine's C bindings for the SOM runtime kernel (ADR-0059)
 *
 * Written for Warpine from the public SOM documentation (SOM Programming
 * Guide and Reference: SOMObject, SOMClass, SOMClassMgr, somResolve,
 * somClassResolve, somDataResolve, somIdFromString, somEnvironmentNew,
 * somDefaultInit, somDestruct).
 * It stands in for the header the SOMobjects Toolkit generates and ships:
 * the classes in this directory were bound by hand, the way the SOM
 * compiler's C emitter binds them, so the Open Watcom SOM samples build
 * unchanged.
 *
 * Every SOM object begins with a pointer to its class's method table; a
 * method is called through the token in its class's ClassData:
 *   ((somTD_Class_method) somResolve(obj, ClassClassData.method))(obj, ...)
 */
#ifndef WARPINE_SOM_H
#define WARPINE_SOM_H

#include <stddef.h>
#include <string.h>

#define SOMLINK   _System
#define SOMDLINK  _System
#ifdef __cplusplus
#define SOMEXTERN extern "C"
#else
#define SOMEXTERN extern
#endif
#define SOM_Scope static

typedef long           integer4;
typedef unsigned long  uinteger4;
typedef char          *zString;
typedef char          *string;
typedef unsigned long  somMToken;
typedef unsigned long  somDToken;
typedef char         **somId;
typedef void SOMLINK   somMethodProc( void );

struct SOMAny_struct;
/* A method table (IBM's layout, ADR-0072): a header, then the procedures
 * in the order of the class's methods. */
typedef struct somMethodTabStruct {
    struct SOMAny_struct *classObject;
    void                 *classInfo;
    char                 *className;
    long                  instanceSize;
    long                  dataAlignment;
    long                  mtabSize;      /* bytes, header included */
    long                  protectedDataOffset;
    somDToken             protectedDataToken;
    void                 *embeddedObjs;
    somMethodProc        *entries[1];
} somMethodTab;

typedef struct SOMAny_struct {
    somMethodTab *mtab;
} SOMAny;

typedef struct somMethodTabList {
    somMethodTab            *mtab;
    struct somMethodTabList *next;
} somMethodTabList, *somMethodTabs;

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


/* In C every object is a SOMAny; in C++ (som.xh) the classes are C++
 * classes whose only member is that first word. */
#ifdef __cplusplus
class SOMObject;
class SOMClass;
class SOMClassMgr;
#else
typedef SOMAny SOMObject;
typedef SOMAny SOMClass;
typedef SOMAny SOMClassMgr;
#endif

typedef struct Environment {
    long  _major;
    void *exception_id;
    void *params;
    long  reserved;
} Environment;

/* ---- SOM functions ---- */
SOMEXTERN SOMClassMgr   * SOMLINK somEnvironmentNew( void );
SOMEXTERN somMethodProc * SOMLINK somResolve( SOMObject *obj, somMToken method );
SOMEXTERN somMethodProc * SOMLINK somResolveByName( SOMObject *obj, char *name );
SOMEXTERN somMethodProc * SOMLINK somClassResolve( SOMClass *cls, somMToken method );
SOMEXTERN void          * SOMLINK somDataResolve( SOMObject *obj, somDToken data );
SOMEXTERN void          * SOMLINK somDataResolveChk( SOMObject *obj, somDToken data );
SOMEXTERN Environment   * SOMLINK somGetGlobalEnvironment( void );
SOMEXTERN somId           SOMLINK somIdFromString( zString s );
SOMEXTERN zString         SOMLINK somStringFromId( somId id );
SOMEXTERN int             SOMLINK somCompareIds( somId a, somId b );
SOMEXTERN int             SOMLINK somIsObj( void *p );
SOMEXTERN int             SOMLINK somPrintf( char *fmt, ... );
SOMEXTERN somMethodProc * SOMLINK somParentNumResolve( somMethodTabs parentMtabs, int parentNum, somMToken method );
SOMEXTERN SOMClass      * SOMLINK somBuildClass( long inheritVars, somStaticClassInfo *sci, long majorVersion, long minorVersion );
SOMEXTERN SOMClass      * SOMLINK SOMObjectNewClass( integer4 majorVersion, integer4 minorVersion );
SOMEXTERN SOMClass      * SOMLINK SOMClassNewClass( integer4 majorVersion, integer4 minorVersion );
/* Warpine's: the metaclass of a new class that asks for `requested` (0:
 * none) and has parent `parent` - a derived metaclass when they clash. */
SOMEXTERN SOMClass      * SOMLINK WrpSomMetaclassFor( SOMClass *requested, SOMClass *parent );

/* ---- Runtime variables ---- */
typedef void * SOMLINK somTD_SOMMalloc( size_t size );
typedef void * SOMLINK somTD_SOMCalloc( size_t count, size_t size );
typedef void * SOMLINK somTD_SOMRealloc( void *p, size_t size );
typedef void   SOMLINK somTD_SOMFree( void *p );
SOMEXTERN somTD_SOMMalloc  * SOMDLINK SOMMalloc;
SOMEXTERN somTD_SOMCalloc  * SOMDLINK SOMCalloc;
SOMEXTERN somTD_SOMRealloc * SOMDLINK SOMRealloc;
SOMEXTERN somTD_SOMFree    * SOMDLINK SOMFree;
SOMEXTERN long SOMDLINK SOM_TraceLevel;
SOMEXTERN long SOMDLINK SOM_WarnLevel;
SOMEXTERN long SOMDLINK SOM_AssertLevel;
/* Class loading, errors and somPrintf's output: a program may replace
 * them. SOMLoadModule loads `fileName` for `className` and calls its
 * `functionName` (or <className>NewClass), returning 0 and the module in
 * *modHandle; a SOM error code's last digit is its severity (9: fatal). */
typedef int  SOMLINK somTD_SOMOutCharRoutine( char c );
typedef int  SOMLINK somTD_SOMLoadModule( zString className, zString fileName, zString functionName,
                                          long majorVersion, long minorVersion, void **modHandle );
typedef int  SOMLINK somTD_SOMDeleteModule( void *modHandle );
typedef void SOMLINK somTD_SOMError( int code, zString fileName, int lineNum );
SOMEXTERN somTD_SOMOutCharRoutine * SOMDLINK SOMOutCharRoutine;
SOMEXTERN somTD_SOMLoadModule     * SOMDLINK SOMLoadModule;
SOMEXTERN somTD_SOMDeleteModule   * SOMDLINK SOMDeleteModule;
SOMEXTERN somTD_SOMError          * SOMDLINK SOMError;
/* somSetOutChar( 0 ) puts the default routine back (Warpine's). */
SOMEXTERN void SOMLINK somSetOutChar( somTD_SOMOutCharRoutine *routine );
#define SOM_Fatal 9
#define SOM_Error( code ) SOMError( (code), __FILE__, __LINE__ )

/* ---- IDL sequences: sequence(T) is a struct of _maximum, _length and a
 * buffer of T pointers (objects are pointers with -maddstar). ---- */
#define sequence( type ) _IDL_SEQUENCE_##type
#define SOM_SEQUENCE_OF( type ) \
    typedef struct { unsigned long _maximum; unsigned long _length; type **_buffer; } _IDL_SEQUENCE_##type

/* ---- The root classes' ClassData (tokens in IBM's release order, ADR-0072)
 * and the class manager ---- */
SOMEXTERN struct SOMObjectClassDataStructure {
    SOMClass *classObject;
    somMToken somInit, somUninit, somFree, somDefaultVCopyInit, somGetClassName,
              somGetClass, somIsA, somRespondsTo, somIsInstanceOf, somGetSize,
              somDumpSelf, somDumpSelfInt, somPrintSelf, somDefaultConstVCopyInit,
              somDispatchV, somDispatchL, somDispatchA, somDispatchD, somDispatch,
              somClassDispatch, somCastObj, somResetObj, somDefaultInit,
              somDestruct, somPrivate1, somPrivate2, somDefaultCopyInit,
              somDefaultConstCopyInit, somDefaultAssign, somDefaultConstAssign,
              somDefaultVAssign, somDefaultConstVAssign;
} SOMDLINK SOMObjectClassData;

SOMEXTERN struct SOMClassClassDataStructure {
    SOMClass *classObject;
    somMToken somNew, somRenew, somInitClass, somClassReady, somGetName,
              somGetParent, somDescendedFrom, somCheckVersion, somFindMethod,
              somFindMethodOk, somSupportsMethod, somGetNumMethods,
              somGetInstanceSize, somGetInstanceOffset, somGetInstancePartSize,
              somGetMethodIndex, somGetNumStaticMethods, somGetPClsMtab,
              somGetClassMtab, somAddStaticMethod, somOverrideSMethod,
              somAddDynamicMethod, somcPrivate0, somGetApplyStub, somFindSMethod,
              somFindSMethodOk, somGetMethodDescriptor, somGetNthMethodInfo,
              somSetClassData, somGetClassData, somNewNoInit, somRenewNoInit,
              somGetInstanceToken, somGetMemberToken, somcPrivate1,
              somGetMethodData, somOverrideMtab, somGetMethodToken, somGetParents,
              somGetPClsMtabs, somInitMIClass, somGetVersionNumbers,
              somLookupMethod, _get_somInstanceDataOffsets, somRenewNoZero,
              somRenewNoInitNoZero, somAllocate, somDeallocate, somGetRdStub,
              somGetNthMethodData, somcPrivate2, somcPrivate3,
              _get_somDirectInitClasses, somcPrivate5, somcPrivate6, somcPrivate7,
              somcPrivate8, somcPrivate9, somcPrivate10, somcPrivate11,
              somcPrivate12, somDefinedMethod, somcPrivate13, somcPrivate14,
              somcPrivate15, somcPrivate16, somcPrivate17, somcPrivate18,
              somcPrivate19, somcPrivate20, _get_somDataAlignment, somcPrivate21,
              somcPrivate22, somcPrivate23, somcPrivate24, somcPrivate25;
} SOMDLINK SOMClassClassData;

SOMEXTERN struct SOMClassMgrClassDataStructure {
    SOMClass *classObject;
    somMToken somFindClsInFile, somFindClass, somClassFromId, somRegisterClass,
              somUnregisterClass, somLocateClassFile, somLoadClassFile,
              somUnloadClassFile, somGetInitFunction, somMergeInto,
              somGetRelatedClasses, somSubstituteClass,
              _get_somInterfaceRepository, _set_somInterfaceRepository,
              _get_somRegisteredClasses, somBeginPersistentClasses,
              somEndPersistentClasses, somcmPrivate1, somcmPrivate2,
              somRegisterClassLibrary, somJoinAffinityGroup,
              somUnregisterClassLibrary;
} SOMDLINK SOMClassMgrClassData;

SOMEXTERN SOMClassMgr * SOMDLINK SOMClassMgrObject;

/* ---- Calling methods ---- */
#define SOM_Resolve( o, cls, m ) \
    ((somTD_##cls##_##m) somResolve( (SOMObject *)(o), cls##ClassData.m ))
#define SOM_ClassResolve( parentcls, cls, m ) \
    ((somTD_##cls##_##m) somClassResolve( (SOMClass *)(parentcls), cls##ClassData.m ))

/* SOMObject */
typedef void      SOMLINK somTP_SOMObject_somInit( SOMObject * );
typedef void      SOMLINK somTP_SOMObject_somUninit( SOMObject * );
typedef void      SOMLINK somTP_SOMObject_somFree( SOMObject * );
typedef SOMClass *SOMLINK somTP_SOMObject_somGetClass( SOMObject * );
typedef zString   SOMLINK somTP_SOMObject_somGetClassName( SOMObject * );
typedef integer4  SOMLINK somTP_SOMObject_somGetSize( SOMObject * );
typedef int       SOMLINK somTP_SOMObject_somIsA( SOMObject *, SOMClass * );
typedef int       SOMLINK somTP_SOMObject_somIsInstanceOf( SOMObject *, SOMClass * );
typedef int       SOMLINK somTP_SOMObject_somRespondsTo( SOMObject *, somId );
typedef SOMObject *SOMLINK somTP_SOMObject_somPrintSelf( SOMObject * );
typedef void      SOMLINK somTP_SOMObject_somDumpSelf( SOMObject *, long );
typedef void      SOMLINK somTP_SOMObject_somDumpSelfInt( SOMObject *, long );
/* SOM 2.1 initializers; the control blocks are not used (pass 0). */
typedef void      SOMLINK somTP_SOMObject_somDefaultInit( SOMObject *, void * );
typedef void      SOMLINK somTP_SOMObject_somDestruct( SOMObject *, unsigned char, void * );
typedef somTP_SOMObject_somInit         *somTD_SOMObject_somInit;
typedef somTP_SOMObject_somUninit       *somTD_SOMObject_somUninit;
typedef somTP_SOMObject_somFree         *somTD_SOMObject_somFree;
typedef somTP_SOMObject_somGetClass     *somTD_SOMObject_somGetClass;
typedef somTP_SOMObject_somGetClassName *somTD_SOMObject_somGetClassName;
typedef somTP_SOMObject_somGetSize      *somTD_SOMObject_somGetSize;
typedef somTP_SOMObject_somIsA          *somTD_SOMObject_somIsA;
typedef somTP_SOMObject_somIsInstanceOf *somTD_SOMObject_somIsInstanceOf;
typedef somTP_SOMObject_somRespondsTo   *somTD_SOMObject_somRespondsTo;
typedef somTP_SOMObject_somPrintSelf    *somTD_SOMObject_somPrintSelf;
typedef somTP_SOMObject_somDumpSelf     *somTD_SOMObject_somDumpSelf;
typedef somTP_SOMObject_somDumpSelfInt  *somTD_SOMObject_somDumpSelfInt;
typedef somTP_SOMObject_somDefaultInit  *somTD_SOMObject_somDefaultInit;
typedef somTP_SOMObject_somDestruct     *somTD_SOMObject_somDestruct;

#define _somInit( o )           SOM_Resolve( o, SOMObject, somInit )( (SOMObject *)(o) )
#define _somUninit( o )         SOM_Resolve( o, SOMObject, somUninit )( (SOMObject *)(o) )
#define _somFree( o )           SOM_Resolve( o, SOMObject, somFree )( (SOMObject *)(o) )
#define _somGetClass( o )       SOM_Resolve( o, SOMObject, somGetClass )( (SOMObject *)(o) )
#define _somGetClassName( o )   SOM_Resolve( o, SOMObject, somGetClassName )( (SOMObject *)(o) )
#define _somGetSize( o )        SOM_Resolve( o, SOMObject, somGetSize )( (SOMObject *)(o) )
#define _somIsA( o, c )         SOM_Resolve( o, SOMObject, somIsA )( (SOMObject *)(o), (c) )
#define _somIsInstanceOf( o, c ) SOM_Resolve( o, SOMObject, somIsInstanceOf )( (SOMObject *)(o), (c) )
#define _somRespondsTo( o, id ) SOM_Resolve( o, SOMObject, somRespondsTo )( (SOMObject *)(o), (id) )
#define _somPrintSelf( o )      SOM_Resolve( o, SOMObject, somPrintSelf )( (SOMObject *)(o) )
#define _somDumpSelf( o, l )    SOM_Resolve( o, SOMObject, somDumpSelf )( (SOMObject *)(o), (l) )
#define _somDumpSelfInt( o, l ) SOM_Resolve( o, SOMObject, somDumpSelfInt )( (SOMObject *)(o), (l) )
#define _somDefaultInit( o, c ) SOM_Resolve( o, SOMObject, somDefaultInit )( (SOMObject *)(o), (c) )
#define _somDestruct( o, f, c ) SOM_Resolve( o, SOMObject, somDestruct )( (SOMObject *)(o), (f), (c) )
/* The long names of SOM 2.x C bindings. */
#define SOMObject_somInit( o )         _somInit( o )
#define SOMObject_somUninit( o )       _somUninit( o )
#define SOMObject_somFree( o )         _somFree( o )
#define SOMObject_somGetClass( o )     _somGetClass( o )
#define SOMObject_somGetClassName( o ) _somGetClassName( o )
#define SOMObject_somIsA( o, c )       _somIsA( o, c )
#define SOMObject_somPrintSelf( o )    _somPrintSelf( o )
#define SOMObject_somDumpSelf( o, l )  _somDumpSelf( o, l )

/* A parent's implementation of a method: in the `n`th parent's table of a
 * class's <Class>CClassData.parentMtab (somidl's bindings). */
#define SOM_ParentNumResolve( cls, n, mtabs, m ) \
    ((somTD_##cls##_##m) somParentNumResolve( (mtabs), (n), cls##ClassData.m ))
#define SOMMethodDebug( c, m ) SOM_MethodDebug( c, m )

/* A parent's implementation of a method: resolved in the parent class. */
#define SOM_ParentResolve( parent, cls, m ) \
    ((somTD_##cls##_##m) somClassResolve( (parent), cls##ClassData.m ))

/* SOMClass */
typedef SOMObject *SOMLINK somTP_SOMClass_somNew( SOMClass * );
typedef SOMObject *SOMLINK somTP_SOMClass_somNewNoInit( SOMClass * );
typedef zString    SOMLINK somTP_SOMClass_somGetName( SOMClass * );
typedef SOMClass  *SOMLINK somTP_SOMClass_somGetParent( SOMClass * );
typedef integer4   SOMLINK somTP_SOMClass_somGetInstanceSize( SOMClass * );
typedef somDToken  SOMLINK somTP_SOMClass_somGetInstanceToken( SOMClass * );
typedef int        SOMLINK somTP_SOMClass_somDescendedFrom( SOMClass *, SOMClass * );
typedef void       SOMLINK somTP_SOMClass_somInitClass( SOMClass *, zString, SOMClass *, integer4,
                                                        integer4, integer4, integer4 );
typedef somMToken  SOMLINK somTP_SOMClass_somAddStaticMethod( SOMClass *, somId, somId,
                                                              somMethodProc *, somMethodProc *,
                                                              somMethodProc * );
typedef void       SOMLINK somTP_SOMClass_somOverrideSMethod( SOMClass *, somId, somMethodProc * );
typedef void       SOMLINK somTP_SOMClass_somClassReady( SOMClass * );
typedef somTP_SOMClass_somNew              *somTD_SOMClass_somNew;
typedef somTP_SOMClass_somNewNoInit        *somTD_SOMClass_somNewNoInit;
typedef somTP_SOMClass_somGetName          *somTD_SOMClass_somGetName;
typedef somTP_SOMClass_somGetParent        *somTD_SOMClass_somGetParent;
typedef somTP_SOMClass_somGetInstanceSize  *somTD_SOMClass_somGetInstanceSize;
typedef somTP_SOMClass_somGetInstanceToken *somTD_SOMClass_somGetInstanceToken;
typedef somTP_SOMClass_somDescendedFrom    *somTD_SOMClass_somDescendedFrom;
typedef somTP_SOMClass_somInitClass        *somTD_SOMClass_somInitClass;
typedef somTP_SOMClass_somAddStaticMethod  *somTD_SOMClass_somAddStaticMethod;
typedef somTP_SOMClass_somOverrideSMethod  *somTD_SOMClass_somOverrideSMethod;
typedef somTP_SOMClass_somClassReady       *somTD_SOMClass_somClassReady;

#define _somNew( c )              SOM_Resolve( c, SOMClass, somNew )( (SOMClass *)(c) )
#define _somNewNoInit( c )        SOM_Resolve( c, SOMClass, somNewNoInit )( (SOMClass *)(c) )
#define _somGetName( c )          SOM_Resolve( c, SOMClass, somGetName )( (SOMClass *)(c) )
#define _somGetParent( c )        SOM_Resolve( c, SOMClass, somGetParent )( (SOMClass *)(c) )
#define _somGetInstanceSize( c )  SOM_Resolve( c, SOMClass, somGetInstanceSize )( (SOMClass *)(c) )
#define _somGetInstanceToken( c ) SOM_Resolve( c, SOMClass, somGetInstanceToken )( (SOMClass *)(c) )
#define _somDescendedFrom( c, p ) SOM_Resolve( c, SOMClass, somDescendedFrom )( (SOMClass *)(c), (p) )
#define _somInitClass( c, n, p, sz, mx, ma, mi ) \
    SOM_Resolve( c, SOMClass, somInitClass )( (SOMClass *)(c), (n), (p), (sz), (mx), (ma), (mi) )
#define _somAddStaticMethod( c, id, desc, m, rd, ap ) \
    SOM_Resolve( c, SOMClass, somAddStaticMethod )( (SOMClass *)(c), (id), (desc), (m), (rd), (ap) )
#define _somOverrideSMethod( c, id, m ) \
    SOM_Resolve( c, SOMClass, somOverrideSMethod )( (SOMClass *)(c), (id), (m) )
#define _somClassReady( c )       SOM_Resolve( c, SOMClass, somClassReady )( (SOMClass *)(c) )

/* SOMClassMgr: a class by name, loaded from its file when it is not
 * there yet (the file: the class's name, or `file`). */
typedef SOMClass *SOMLINK somTP_SOMClassMgr_somFindClass( SOMClassMgr *, somId, long, long );
typedef SOMClass *SOMLINK somTP_SOMClassMgr_somFindClsInFile( SOMClassMgr *, somId, long, long, zString );
typedef zString   SOMLINK somTP_SOMClassMgr_somLocateClassFile( SOMClassMgr *, somId, long, long );
typedef SOMClass *SOMLINK somTP_SOMClassMgr_somLoadClassFile( SOMClassMgr *, somId, long, long, zString );
typedef somTP_SOMClassMgr_somFindClass       *somTD_SOMClassMgr_somFindClass;
typedef somTP_SOMClassMgr_somFindClsInFile   *somTD_SOMClassMgr_somFindClsInFile;
typedef somTP_SOMClassMgr_somLocateClassFile *somTD_SOMClassMgr_somLocateClassFile;
typedef somTP_SOMClassMgr_somLoadClassFile   *somTD_SOMClassMgr_somLoadClassFile;
/* Forget a class; unload the module that made it (with its classes). */
typedef long SOMLINK somTP_SOMClassMgr_somUnregisterClass( SOMClassMgr *, SOMClass * );
typedef long SOMLINK somTP_SOMClassMgr_somUnloadClassFile( SOMClassMgr *, SOMClass * );
typedef somTP_SOMClassMgr_somUnregisterClass *somTD_SOMClassMgr_somUnregisterClass;
typedef somTP_SOMClassMgr_somUnloadClassFile *somTD_SOMClassMgr_somUnloadClassFile;
#define _somUnregisterClass( m, c ) \
    SOM_Resolve( m, SOMClassMgr, somUnregisterClass )( (SOMClassMgr *)(m), (SOMClass *)(c) )
#define _somUnloadClassFile( m, c ) \
    SOM_Resolve( m, SOMClassMgr, somUnloadClassFile )( (SOMClassMgr *)(m), (SOMClass *)(c) )
#define _somFindClass( m, id, ma, mi ) \
    SOM_Resolve( m, SOMClassMgr, somFindClass )( (SOMClassMgr *)(m), (id), (ma), (mi) )
#define _somFindClsInFile( m, id, ma, mi, f ) \
    SOM_Resolve( m, SOMClassMgr, somFindClsInFile )( (SOMClassMgr *)(m), (id), (ma), (mi), (f) )
#define _somLocateClassFile( m, id, ma, mi ) \
    SOM_Resolve( m, SOMClassMgr, somLocateClassFile )( (SOMClassMgr *)(m), (id), (ma), (mi) )
#define _somLoadClassFile( m, id, ma, mi, f ) \
    SOM_Resolve( m, SOMClassMgr, somLoadClassFile )( (SOMClassMgr *)(m), (id), (ma), (mi), (f) )

/* What the C emitter's .ih files share: building a class, an instance of
 * the metaclass it asks for (0: its parent's), or of a metaclass derived
 * from that and its parent's. */
#define SOM_NEW_META_CLASS( cls, name, parent, meta, datasize, nmethods, major, minor ) \
    ( somEnvironmentNew(), \
      (cls) = (SOMClass *)_somNew( WrpSomMetaclassFor( (meta), (parent) ) ), \
      _somInitClass( (cls), (name), (parent), (datasize), (nmethods), (major), (minor) ) )
#define SOM_NEW_CLASS( cls, name, parent, datasize, nmethods, major, minor ) \
    SOM_NEW_META_CLASS( cls, name, parent, (SOMClass *)0, datasize, nmethods, major, minor )
#define SOM_ADD_METHOD( cls, name, proc ) \
    _somAddStaticMethod( (cls), somIdFromString( name ), somIdFromString( "" ), \
                         (somMethodProc *)(proc), (somMethodProc *)0, (somMethodProc *)0 )
#define SOM_OVERRIDE( cls, name, proc ) \
    _somOverrideSMethod( (cls), somIdFromString( name ), (somMethodProc *)(proc) )

/* Method tracing: a line per method entered while SOM_TraceLevel is set. */
#define SOM_NoTrace( c, m ) ((void)0)
#define SOM_MethodDebug( c, m ) \
    do { if( SOM_TraceLevel ) somPrintf( "\"%s\": %s\n", (c), (m) ); } while( 0 )

#endif
