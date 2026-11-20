/* sci.h - the static class information somBuildClass takes (ADR-0073),
 * declared from the layout the OS/2 Warp 4 Toolkit's somapi.h gives:
 * layout version 3. Code the SOM compiler generates fills these
 * structures statically and passes them to somBuildClass. */
#ifndef SCI_H
#define SCI_H
#include <som.h>

typedef struct {
    somMToken *classData;        /* where the method's token goes */
    somId     *methodId;
    somId     *methodDescriptor;
    somMethodProc *method;
    somMethodProc *redispatchStub;
    somMethodProc *applyStub;
} sciStaticMethod;

typedef struct {
    somId         *methodId;     /* may be "Class::method" */
    somMethodProc *method;
} sciOverrideMethod;

typedef struct {
    somId     *methodId;
    long       parentNum;        /* from 1 */
    somMToken *mToken;
} sciInheritedMethod;

typedef struct {
    somDToken *classData;        /* where the data token goes */
    char      *basisForDataOffset;
} sciNonInternalData;

typedef struct {
    somMethodProc **classData;
    somMethodProc  *pEntry;
} sciProcMethod;

typedef void SOMLINK sciClassInitFunc( SOMClass * );

typedef struct {
    unsigned long layoutVersion;
    unsigned long numStaticMethods, numStaticOverrides, numNonInternalData;
    unsigned long numProcMethods, numVarargsFuncs;
    unsigned long majorVersion, minorVersion;
    unsigned long instanceDataSize, maxMethods, numParents;
    somId         classId, explicitMetaId;
    long          implicitParentMeta;
    somId        *parents;
    void         *cds, *ccds;
    sciStaticMethod    *smt;
    sciOverrideMethod  *omt;
    char               *nitReferenceBase;
    sciNonInternalData *nit;
    sciProcMethod      *pmt;
    void               *vft;
    sciClassInitFunc   *cif;
    long          dataAlignment;                       /* layout 2 */
    long          numDirectInitClasses;                /* layout 3 */
    somId        *directInitClasses;
    unsigned long numMethods;
    void         *mt;
    unsigned long protectedDataOffset, somSCIVersion, numInheritedMethods;
    sciInheritedMethod *imt;
    unsigned long numClassDataEntries;
    somId        *classDataEntryNames;
    unsigned long numMigratedMethods;
    void         *mmt;
    unsigned long numInitializers;
    somId        *initializers;
    unsigned long somDTSClass;
    void         *dsci;
} sciStaticClassInfo;

/* CClassData: parentMtab (the class's own table, then the parents'). */
typedef struct sciMtabList {
    somMethodTab       *mtab;
    struct sciMtabList *next;
} sciMtabList;
typedef struct {
    somMethodTab *mtab;
    sciMtabList  *next;
    SOMClass     *classObject;
    void         *renewThunk;
    long          instanceSize;
} sciParentMtab;
typedef struct {
    sciParentMtab *parentMtab;
    somDToken      instanceDataToken;
} sciCClassData;

SOMEXTERN SOMClass      * SOMLINK somBuildClass( long inheritVars, sciStaticClassInfo *sci,
                                                 long majorVersion, long minorVersion );
SOMEXTERN somMethodProc * SOMLINK somParentNumResolve( sciParentMtab *mtabs, int n, somMToken method );
#endif
