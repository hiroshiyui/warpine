/*
 * graduate.h - C usage bindings of class GraduateStudent (graduate.csc of the Open Watcom SOM
 * samples), written by hand for Warpine as the SOM compiler would emit
 * them (ADR-0059).
 */
#ifndef SOM_GraduateStudent_h
#define SOM_GraduateStudent_h

#include "student.h"

typedef Student GraduateStudent;

typedef void SOMLINK somTP_GraduateStudent_setUpGraduateStudent( GraduateStudent *somSelf, char *id, char *name, char *thesis, char *degree );
typedef somTP_GraduateStudent_setUpGraduateStudent *somTD_GraduateStudent_setUpGraduateStudent;

SOMEXTERN SOMClass * SOMLINK GraduateStudentNewClass( integer4 major, integer4 minor );

SOMEXTERN struct GraduateStudentClassDataStructure {
    SOMClass *classObject;
    somMToken setUpGraduateStudent;
} SOMDLINK GraduateStudentClassData;

#define _GraduateStudent  (GraduateStudentClassData.classObject ? GraduateStudentClassData.classObject : GraduateStudentNewClass( 0, 0 ))
#define GraduateStudentNew()  ((GraduateStudent *)_somNew( _GraduateStudent ))

#define _setUpGraduateStudent( somSelf, id, name, thesis, degree ) \
    SOM_Resolve( somSelf, GraduateStudent, setUpGraduateStudent )( (GraduateStudent *)(somSelf), id, name, thesis, degree )

#endif
