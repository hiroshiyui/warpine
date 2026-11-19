/*
 * student.h - C usage bindings of class Student (student.csc of the Open
 * Watcom SOM samples), written by hand for Warpine as the SOM compiler
 * would emit them (ADR-0059).
 */
#ifndef SOM_Student_h
#define SOM_Student_h

#include <som.h>

typedef SOMObject Student;

typedef void    SOMLINK somTP_Student_setUpStudent( Student *somSelf, char *id, char *name );
typedef void    SOMLINK somTP_Student_printStudentInfo( Student *somSelf );
typedef char *  SOMLINK somTP_Student_getStudentType( Student *somSelf );
typedef char *  SOMLINK somTP_Student_getStudentId( Student *somSelf );
typedef somTP_Student_setUpStudent     *somTD_Student_setUpStudent;
typedef somTP_Student_printStudentInfo *somTD_Student_printStudentInfo;
typedef somTP_Student_getStudentType   *somTD_Student_getStudentType;
typedef somTP_Student_getStudentId     *somTD_Student_getStudentId;

SOMEXTERN SOMClass * SOMLINK StudentNewClass( integer4 major, integer4 minor );

SOMEXTERN struct StudentClassDataStructure {
    SOMClass *classObject;
    somMToken setUpStudent, printStudentInfo, getStudentType, getStudentId;
} SOMDLINK StudentClassData;

#define _Student  (StudentClassData.classObject ? StudentClassData.classObject : StudentNewClass( 0, 0 ))
#define StudentNew()  ((Student *)_somNew( _Student ))

#define _setUpStudent( somSelf, id, name ) \
    SOM_Resolve( somSelf, Student, setUpStudent )( (Student *)(somSelf), (id), (name) )
#define _printStudentInfo( somSelf ) \
    SOM_Resolve( somSelf, Student, printStudentInfo )( (Student *)(somSelf) )
#define _getStudentType( somSelf ) \
    SOM_Resolve( somSelf, Student, getStudentType )( (Student *)(somSelf) )
#define _getStudentId( somSelf ) \
    SOM_Resolve( somSelf, Student, getStudentId )( (Student *)(somSelf) )

#endif
