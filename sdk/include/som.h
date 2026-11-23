/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * som.h - SOM for C programs built with GCC or clang (ADR-0122)
 *
 * Written for Warpine from IBM's public SOM documentation (the System
 * Object Model Programming Reference and Guide; the root classes' release
 * orders as ADR-0072 records them); nothing is copied from IBM's headers.
 * It declares the kernel (somapi.h), the binding macros and Warpine's
 * initializer protocol (somcdev.h), and the root classes SOMObject,
 * SOMClass and SOMClassMgr: their ClassData (tokens in IBM's release
 * order), method types and the _method( obj, ... ) macros.
 */
#ifndef WARPINE_SOM_H
#define WARPINE_SOM_H

#include <somapi.h>
/* SOM sources expect som.h to bring in the string functions. */
#include <string.h>

/* ---- IDL sequences: sequence(T) is a struct of _maximum, _length and a
 * buffer of T. ---- */
#define sequence( type ) _IDL_SEQUENCE_##type
#define SOM_SEQUENCE_OF( type ) \
    typedef struct { unsigned long _maximum; unsigned long _length; type *_buffer; } _IDL_SEQUENCE_##type

/* ---- The root classes' ClassData (tokens in IBM's release order, ADR-0072)
 * and the class manager ---- */
struct SOMObjectClassDataStructure {
    SOMClass *classObject;
    somMToken somInit, somUninit, somFree, somDefaultVCopyInit, somGetClassName,
              somGetClass, somIsA, somRespondsTo, somIsInstanceOf, somGetSize,
              somDumpSelf, somDumpSelfInt, somPrintSelf, somDefaultConstVCopyInit,
              somDispatchV, somDispatchL, somDispatchA, somDispatchD, somDispatch,
              somClassDispatch, somCastObj, somResetObj, somDefaultInit,
              somDestruct, somPrivate1, somPrivate2, somDefaultCopyInit,
              somDefaultConstCopyInit, somDefaultAssign, somDefaultConstAssign,
              somDefaultVAssign, somDefaultConstVAssign;
};
#define SOMObjectClassData WRP_SOM_DATA( struct SOMObjectClassDataStructure, 14 )

struct SOMClassClassDataStructure {
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
};
#define SOMClassClassData WRP_SOM_DATA( struct SOMClassClassDataStructure, 3 )

struct SOMClassMgrClassDataStructure {
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
};
#define SOMClassMgrClassData WRP_SOM_DATA( struct SOMClassMgrClassDataStructure, 6 )

/* <Class>CClassData: parentMtab, then the instance-data token. */
struct SOMObjectCClassDataStructure   { somMethodTabs parentMtab; somDToken instanceDataToken; };
struct SOMClassCClassDataStructure    { somMethodTabs parentMtab; somDToken instanceDataToken; };
struct SOMClassMgrCClassDataStructure { somMethodTabs parentMtab; somDToken instanceDataToken; };
#define SOMClassCClassData    WRP_SOM_DATA( struct SOMClassCClassDataStructure, 2 )
#define SOMClassMgrCClassData WRP_SOM_DATA( struct SOMClassMgrCClassDataStructure, 5 )
#define SOMObjectCClassData   WRP_SOM_DATA( struct SOMObjectCClassDataStructure, 13 )

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
/* SOM 2.1 initializers and destructors; the control block is Warpine's
 * (somapi.h), 0 from somNew / somFree. */
typedef void      SOMLINK somTP_SOMObject_somDefaultInit( SOMObject *, somInitCtrl * );
typedef void      SOMLINK somTP_SOMObject_somDestruct( SOMObject *, octet, somDestructCtrl * );
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
/* The copy and assign initializers: without a control block, SOMObject's
 * copy the whole instance shallowly (Warpine's default, ADR-0122). */
typedef void       SOMLINK somTP_SOMObject_somDefaultCopyInit( SOMObject *, somInitCtrl *, SOMObject * );
typedef SOMObject *SOMLINK somTP_SOMObject_somDefaultAssign( SOMObject *, somAssignCtrl *, SOMObject * );
typedef somTP_SOMObject_somDefaultCopyInit *somTD_SOMObject_somDefaultCopyInit;
typedef somTP_SOMObject_somDefaultAssign   *somTD_SOMObject_somDefaultAssign;
/* somCastObj: the object answers as an ancestor until somResetObj. */
typedef boolean    SOMLINK somTP_SOMObject_somCastObj( SOMObject *, SOMClass * );
typedef boolean    SOMLINK somTP_SOMObject_somResetObj( SOMObject * );
typedef somTP_SOMObject_somCastObj  *somTD_SOMObject_somCastObj;
typedef somTP_SOMObject_somResetObj *somTD_SOMObject_somResetObj;

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
#define _somCastObj( o, c )     SOM_Resolve( o, SOMObject, somCastObj )( (SOMObject *)(o), (SOMClass *)(c) )
#define _somResetObj( o )       SOM_Resolve( o, SOMObject, somResetObj )( (SOMObject *)(o) )
#define _somDefaultCopyInit( o, c, from ) \
    SOM_Resolve( o, SOMObject, somDefaultCopyInit )( (SOMObject *)(o), (c), (SOMObject *)(from) )
#define _somDefaultAssign( o, c, from ) \
    SOM_Resolve( o, SOMObject, somDefaultAssign )( (SOMObject *)(o), (c), (SOMObject *)(from) )
/* The long names of SOM 2.x C bindings. */
#define SOMObject_somInit( o )         _somInit( o )
#define SOMObject_somUninit( o )       _somUninit( o )
#define SOMObject_somFree( o )         _somFree( o )
#define SOMObject_somGetClass( o )     _somGetClass( o )
#define SOMObject_somGetClassName( o ) _somGetClassName( o )
#define SOMObject_somIsA( o, c )       _somIsA( o, c )
#define SOMObject_somPrintSelf( o )    _somPrintSelf( o )
#define SOMObject_somDumpSelf( o, l )  _somDumpSelf( o, l )

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
typedef void       SOMLINK somTP_SOMClass_somDeallocate( SOMClass *, string );
typedef boolean    SOMLINK somTP_SOMClass_somCheckVersion( SOMClass *, long, long );
/* Objects in the caller's memory, and introspection. */
typedef SOMObject *SOMLINK somTP_SOMClass_somRenew( SOMClass *, somToken );
typedef somDToken  SOMLINK somTP_SOMClass_somGetMemberToken( SOMClass *, long, somDToken );
typedef long       SOMLINK somTP_SOMClass_somGetMethodIndex( SOMClass *, somId );
typedef somId      SOMLINK somTP_SOMClass_somGetNthMethodInfo( SOMClass *, long, somId * );
typedef boolean    SOMLINK somTP_SOMClass_somGetMethodData( SOMClass *, somId, somMethodData * );
typedef boolean    SOMLINK somTP_SOMClass_somGetNthMethodData( SOMClass *, long, somMethodData * );
typedef void       SOMLINK somTP_SOMClass_somInitMIClass( SOMClass *, long, zString, SOMClassSequence, long, long,
                                                          long, long, long );
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
typedef somTP_SOMClass_somDeallocate       *somTD_SOMClass_somDeallocate;
typedef somTP_SOMClass_somCheckVersion     *somTD_SOMClass_somCheckVersion;
typedef somTP_SOMClass_somRenew            *somTD_SOMClass_somRenew;
typedef somTP_SOMClass_somRenew            *somTD_SOMClass_somRenewNoInit;
typedef somTP_SOMClass_somRenew            *somTD_SOMClass_somRenewNoZero;
typedef somTP_SOMClass_somRenew            *somTD_SOMClass_somRenewNoInitNoZero;
typedef somTP_SOMClass_somGetMemberToken   *somTD_SOMClass_somGetMemberToken;
typedef somTP_SOMClass_somGetMethodIndex   *somTD_SOMClass_somGetMethodIndex;
typedef somTP_SOMClass_somGetNthMethodInfo *somTD_SOMClass_somGetNthMethodInfo;
typedef somTP_SOMClass_somGetMethodData    *somTD_SOMClass_somGetMethodData;
typedef somTP_SOMClass_somGetNthMethodData *somTD_SOMClass_somGetNthMethodData;
typedef somTP_SOMClass_somInitMIClass      *somTD_SOMClass_somInitMIClass;

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
#define _somInitMIClass( c, iv, n, ps, sz, al, mx, ma, mi ) \
    SOM_Resolve( c, SOMClass, somInitMIClass )( (SOMClass *)(c), (iv), (n), (ps), (sz), (al), (mx), (ma), (mi) )
#define _somRenew( c, p )             SOM_Resolve( c, SOMClass, somRenew )( (SOMClass *)(c), (somToken)(p) )
#define _somRenewNoInit( c, p )       SOM_Resolve( c, SOMClass, somRenewNoInit )( (SOMClass *)(c), (somToken)(p) )
#define _somRenewNoZero( c, p )       SOM_Resolve( c, SOMClass, somRenewNoZero )( (SOMClass *)(c), (somToken)(p) )
#define _somRenewNoInitNoZero( c, p ) SOM_Resolve( c, SOMClass, somRenewNoInitNoZero )( (SOMClass *)(c), (somToken)(p) )
#define _somGetMemberToken( c, off, t ) SOM_Resolve( c, SOMClass, somGetMemberToken )( (SOMClass *)(c), (off), (t) )
#define _somGetMethodIndex( c, id )   SOM_Resolve( c, SOMClass, somGetMethodIndex )( (SOMClass *)(c), (id) )
#define _somGetNthMethodInfo( c, n, d ) SOM_Resolve( c, SOMClass, somGetNthMethodInfo )( (SOMClass *)(c), (n), (d) )
#define _somGetMethodData( c, id, md ) SOM_Resolve( c, SOMClass, somGetMethodData )( (SOMClass *)(c), (id), (md) )
#define _somGetNthMethodData( c, n, md ) SOM_Resolve( c, SOMClass, somGetNthMethodData )( (SOMClass *)(c), (n), (md) )
#define _somCheckVersion( c, ma, mi ) SOM_Resolve( c, SOMClass, somCheckVersion )( (SOMClass *)(c), (ma), (mi) )
#define _somDeallocate( c, p )    SOM_Resolve( c, SOMClass, somDeallocate )( (SOMClass *)(c), (string)(p) )

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
typedef zString SOMLINK somTP_SOMClassMgr_somGetInitFunction( SOMClassMgr * );
typedef somTP_SOMClassMgr_somGetInitFunction *somTD_SOMClassMgr_somGetInitFunction;
#define _somGetInitFunction( m ) SOM_Resolve( m, SOMClassMgr, somGetInitFunction )( (SOMClassMgr *)(m) )
#define _somFindClass( m, id, ma, mi ) \
    SOM_Resolve( m, SOMClassMgr, somFindClass )( (SOMClassMgr *)(m), (id), (ma), (mi) )
#define _somFindClsInFile( m, id, ma, mi, f ) \
    SOM_Resolve( m, SOMClassMgr, somFindClsInFile )( (SOMClassMgr *)(m), (id), (ma), (mi), (f) )
#define _somLocateClassFile( m, id, ma, mi ) \
    SOM_Resolve( m, SOMClassMgr, somLocateClassFile )( (SOMClassMgr *)(m), (id), (ma), (mi) )
#define _somLoadClassFile( m, id, ma, mi, f ) \
    SOM_Resolve( m, SOMClassMgr, somLoadClassFile )( (SOMClassMgr *)(m), (id), (ma), (mi), (f) )

/* The binding macros and Warpine's initializer protocol, which uses the
 * method types above. */
#include <somcdev.h>

#endif
