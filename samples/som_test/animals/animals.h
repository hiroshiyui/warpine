/*
 * animals.h - C usage bindings of module Animals (animals.idl of the Open
 * Watcom SOM samples), written by hand for Warpine as the SOM compiler
 * would emit them with -maddstar (ADR-0059, ADR-0060).
 */
#ifndef SOM_Animals_h
#define SOM_Animals_h

#include <som.h>

typedef SOMObject Animals_M_Animal;
typedef SOMObject Animals_Animal;
typedef SOMObject Animals_Kennel;
typedef SOMObject Animals_Dog;
typedef SOMObject Animals_BigDog;
typedef SOMObject Animals_LittleDog;

SOM_SEQUENCE_OF( Animals_Animal );
typedef sequence( Animals_Animal ) Animals_Animal_AnimalInstances;

/* ---- Animals::M_Animal ---- */
#define Animals_M_Animal_MajorVersion 0
#define Animals_M_Animal_MinorVersion 0
SOMEXTERN SOMClass * SOMLINK Animals_M_AnimalNewClass( integer4 major, integer4 minor );
SOMEXTERN struct Animals_M_AnimalClassDataStructure {
    SOMClass *classObject;
    somMToken _get_instances;
    somMToken _set_instances;
    somMToken newAnimal;
    somMToken addInstance;
    somMToken deleteInstance;
} SOMDLINK Animals_M_AnimalClassData;
#define _Animals_M_Animal Animals_M_AnimalClassData.classObject
typedef _IDL_SEQUENCE_Animals_Animal SOMLINK somTP_Animals_M_Animal__get_instances( Animals_M_Animal *somSelf, Environment *ev );
typedef somTP_Animals_M_Animal__get_instances *somTD_Animals_M_Animal__get_instances;
#define Animals_M_Animal__get_instances( somSelf, ev ) \
    SOM_Resolve( somSelf, Animals_M_Animal, _get_instances )( (Animals_M_Animal *)(somSelf), (ev) )
#define __get_instances Animals_M_Animal__get_instances
typedef void SOMLINK somTP_Animals_M_Animal__set_instances( Animals_M_Animal *somSelf, Environment *ev, _IDL_SEQUENCE_Animals_Animal instances );
typedef somTP_Animals_M_Animal__set_instances *somTD_Animals_M_Animal__set_instances;
#define Animals_M_Animal__set_instances( somSelf, ev, instances ) \
    SOM_Resolve( somSelf, Animals_M_Animal, _set_instances )( (Animals_M_Animal *)(somSelf), (ev), (instances) )
#define __set_instances Animals_M_Animal__set_instances
typedef Animals_Animal * SOMLINK somTP_Animals_M_Animal_newAnimal( Animals_M_Animal *somSelf, Environment *ev, string name, string sound );
typedef somTP_Animals_M_Animal_newAnimal *somTD_Animals_M_Animal_newAnimal;
#define Animals_M_Animal_newAnimal( somSelf, ev, name, sound ) \
    SOM_Resolve( somSelf, Animals_M_Animal, newAnimal )( (Animals_M_Animal *)(somSelf), (ev), (name), (sound) )
#define _newAnimal Animals_M_Animal_newAnimal
typedef void SOMLINK somTP_Animals_M_Animal_addInstance( Animals_M_Animal *somSelf, Environment *ev, Animals_Animal *obj );
typedef somTP_Animals_M_Animal_addInstance *somTD_Animals_M_Animal_addInstance;
#define Animals_M_Animal_addInstance( somSelf, ev, obj ) \
    SOM_Resolve( somSelf, Animals_M_Animal, addInstance )( (Animals_M_Animal *)(somSelf), (ev), (obj) )
#define _addInstance Animals_M_Animal_addInstance
typedef void SOMLINK somTP_Animals_M_Animal_deleteInstance( Animals_M_Animal *somSelf, Environment *ev, Animals_Animal *obj );
typedef somTP_Animals_M_Animal_deleteInstance *somTD_Animals_M_Animal_deleteInstance;
#define Animals_M_Animal_deleteInstance( somSelf, ev, obj ) \
    SOM_Resolve( somSelf, Animals_M_Animal, deleteInstance )( (Animals_M_Animal *)(somSelf), (ev), (obj) )
#define _deleteInstance Animals_M_Animal_deleteInstance

/* ---- Animals::Animal ---- */
#define Animals_Animal_MajorVersion 0
#define Animals_Animal_MinorVersion 0
SOMEXTERN SOMClass * SOMLINK Animals_AnimalNewClass( integer4 major, integer4 minor );
SOMEXTERN struct Animals_AnimalClassDataStructure {
    SOMClass *classObject;
    somMToken _get_name;
    somMToken _set_name;
    somMToken _get_sound;
    somMToken _set_sound;
    somMToken _get_genus;
    somMToken _get_species;
    somMToken talk;
    somMToken display;
} SOMDLINK Animals_AnimalClassData;
#define _Animals_Animal Animals_AnimalClassData.classObject
typedef string SOMLINK somTP_Animals_Animal__get_name( Animals_Animal *somSelf, Environment *ev );
typedef somTP_Animals_Animal__get_name *somTD_Animals_Animal__get_name;
#define Animals_Animal__get_name( somSelf, ev ) \
    SOM_Resolve( somSelf, Animals_Animal, _get_name )( (Animals_Animal *)(somSelf), (ev) )
#define __get_name Animals_Animal__get_name
typedef void SOMLINK somTP_Animals_Animal__set_name( Animals_Animal *somSelf, Environment *ev, string name );
typedef somTP_Animals_Animal__set_name *somTD_Animals_Animal__set_name;
#define Animals_Animal__set_name( somSelf, ev, name ) \
    SOM_Resolve( somSelf, Animals_Animal, _set_name )( (Animals_Animal *)(somSelf), (ev), (name) )
#define __set_name Animals_Animal__set_name
typedef string SOMLINK somTP_Animals_Animal__get_sound( Animals_Animal *somSelf, Environment *ev );
typedef somTP_Animals_Animal__get_sound *somTD_Animals_Animal__get_sound;
#define Animals_Animal__get_sound( somSelf, ev ) \
    SOM_Resolve( somSelf, Animals_Animal, _get_sound )( (Animals_Animal *)(somSelf), (ev) )
#define __get_sound Animals_Animal__get_sound
typedef void SOMLINK somTP_Animals_Animal__set_sound( Animals_Animal *somSelf, Environment *ev, string sound );
typedef somTP_Animals_Animal__set_sound *somTD_Animals_Animal__set_sound;
#define Animals_Animal__set_sound( somSelf, ev, sound ) \
    SOM_Resolve( somSelf, Animals_Animal, _set_sound )( (Animals_Animal *)(somSelf), (ev), (sound) )
#define __set_sound Animals_Animal__set_sound
typedef string SOMLINK somTP_Animals_Animal__get_genus( Animals_Animal *somSelf, Environment *ev );
typedef somTP_Animals_Animal__get_genus *somTD_Animals_Animal__get_genus;
#define Animals_Animal__get_genus( somSelf, ev ) \
    SOM_Resolve( somSelf, Animals_Animal, _get_genus )( (Animals_Animal *)(somSelf), (ev) )
#define __get_genus Animals_Animal__get_genus
typedef string SOMLINK somTP_Animals_Animal__get_species( Animals_Animal *somSelf, Environment *ev );
typedef somTP_Animals_Animal__get_species *somTD_Animals_Animal__get_species;
#define Animals_Animal__get_species( somSelf, ev ) \
    SOM_Resolve( somSelf, Animals_Animal, _get_species )( (Animals_Animal *)(somSelf), (ev) )
#define __get_species Animals_Animal__get_species
typedef void SOMLINK somTP_Animals_Animal_talk( Animals_Animal *somSelf, Environment *ev );
typedef somTP_Animals_Animal_talk *somTD_Animals_Animal_talk;
#define Animals_Animal_talk( somSelf, ev ) \
    SOM_Resolve( somSelf, Animals_Animal, talk )( (Animals_Animal *)(somSelf), (ev) )
#define _talk Animals_Animal_talk
typedef void SOMLINK somTP_Animals_Animal_display( Animals_Animal *somSelf, Environment *ev );
typedef somTP_Animals_Animal_display *somTD_Animals_Animal_display;
#define Animals_Animal_display( somSelf, ev ) \
    SOM_Resolve( somSelf, Animals_Animal, display )( (Animals_Animal *)(somSelf), (ev) )
#define _display Animals_Animal_display

/* ---- Animals::Kennel ---- */
#define Animals_Kennel_MajorVersion 0
#define Animals_Kennel_MinorVersion 0
SOMEXTERN SOMClass * SOMLINK Animals_KennelNewClass( integer4 major, integer4 minor );
SOMEXTERN struct Animals_KennelClassDataStructure {
    SOMClass *classObject;
    somMToken newDog;
} SOMDLINK Animals_KennelClassData;
#define _Animals_Kennel Animals_KennelClassData.classObject
typedef Animals_Dog * SOMLINK somTP_Animals_Kennel_newDog( Animals_Kennel *somSelf, Environment *ev, string name, string sound, string breed, string color );
typedef somTP_Animals_Kennel_newDog *somTD_Animals_Kennel_newDog;
#define Animals_Kennel_newDog( somSelf, ev, name, sound, breed, color ) \
    SOM_Resolve( somSelf, Animals_Kennel, newDog )( (Animals_Kennel *)(somSelf), (ev), (name), (sound), (breed), (color) )
#define _newDog Animals_Kennel_newDog

/* ---- Animals::Dog ---- */
#define Animals_Dog_MajorVersion 0
#define Animals_Dog_MinorVersion 0
SOMEXTERN SOMClass * SOMLINK Animals_DogNewClass( integer4 major, integer4 minor );
SOMEXTERN struct Animals_DogClassDataStructure {
    SOMClass *classObject;
    somMToken _get_breed;
    somMToken _set_breed;
    somMToken _get_color;
    somMToken _set_color;
} SOMDLINK Animals_DogClassData;
#define _Animals_Dog Animals_DogClassData.classObject
typedef string SOMLINK somTP_Animals_Dog__get_breed( Animals_Dog *somSelf, Environment *ev );
typedef somTP_Animals_Dog__get_breed *somTD_Animals_Dog__get_breed;
#define Animals_Dog__get_breed( somSelf, ev ) \
    SOM_Resolve( somSelf, Animals_Dog, _get_breed )( (Animals_Dog *)(somSelf), (ev) )
#define __get_breed Animals_Dog__get_breed
typedef void SOMLINK somTP_Animals_Dog__set_breed( Animals_Dog *somSelf, Environment *ev, string breed );
typedef somTP_Animals_Dog__set_breed *somTD_Animals_Dog__set_breed;
#define Animals_Dog__set_breed( somSelf, ev, breed ) \
    SOM_Resolve( somSelf, Animals_Dog, _set_breed )( (Animals_Dog *)(somSelf), (ev), (breed) )
#define __set_breed Animals_Dog__set_breed
typedef string SOMLINK somTP_Animals_Dog__get_color( Animals_Dog *somSelf, Environment *ev );
typedef somTP_Animals_Dog__get_color *somTD_Animals_Dog__get_color;
#define Animals_Dog__get_color( somSelf, ev ) \
    SOM_Resolve( somSelf, Animals_Dog, _get_color )( (Animals_Dog *)(somSelf), (ev) )
#define __get_color Animals_Dog__get_color
typedef void SOMLINK somTP_Animals_Dog__set_color( Animals_Dog *somSelf, Environment *ev, string color );
typedef somTP_Animals_Dog__set_color *somTD_Animals_Dog__set_color;
#define Animals_Dog__set_color( somSelf, ev, color ) \
    SOM_Resolve( somSelf, Animals_Dog, _set_color )( (Animals_Dog *)(somSelf), (ev), (color) )
#define __set_color Animals_Dog__set_color

/* ---- Animals::BigDog ---- */
#define Animals_BigDog_MajorVersion 0
#define Animals_BigDog_MinorVersion 0
SOMEXTERN SOMClass * SOMLINK Animals_BigDogNewClass( integer4 major, integer4 minor );
SOMEXTERN struct Animals_BigDogClassDataStructure {
    SOMClass *classObject;
} SOMDLINK Animals_BigDogClassData;
#define _Animals_BigDog Animals_BigDogClassData.classObject

/* ---- Animals::LittleDog ---- */
#define Animals_LittleDog_MajorVersion 0
#define Animals_LittleDog_MinorVersion 0
SOMEXTERN SOMClass * SOMLINK Animals_LittleDogNewClass( integer4 major, integer4 minor );
SOMEXTERN struct Animals_LittleDogClassDataStructure {
    SOMClass *classObject;
} SOMDLINK Animals_LittleDogClassData;
#define _Animals_LittleDog Animals_LittleDogClassData.classObject

#endif
