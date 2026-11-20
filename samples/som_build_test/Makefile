# samples/som_build_test/Makefile
#
# Classes built by somBuildClass from static class information (ADR-0073):
# somb.exe builds Animal and Dog; bird.dll holds Bird, which somFindClass
# loads. Built against the hand-written som.h of samples/som_test.

WATCOM_ROOT = $(CURDIR)/../../vendor/watcom
WCC386      = $(WATCOM_ROOT)/binl64/wcc386
WLINK       = $(WATCOM_ROOT)/binl64/wlink
INC         = $(CURDIR)/../som_test/include

export WATCOM  = $(WATCOM_ROOT)
export INCLUDE = $(WATCOM_ROOT)/h/os2:$(WATCOM_ROOT)/h
export PATH   := $(WATCOM_ROOT)/binl64:$(PATH)

all: somb.exe bird.dll

somb.obj: somb.c sci.h $(INC)/som.h
	$(WCC386) -zq -bt=os2 -I$(INC) -fo=$@ $<
bird.obj: bird.c sci.h $(INC)/som.h
	$(WCC386) -zq -bt=os2 -bd -I$(INC) -fo=$@ $<

somb.exe: somb.obj somb.lnk ../som_test/som.lnk
	$(WLINK) name $@ system os2v2 option quiet @../som_test/som.lnk @somb.lnk file somb.obj
bird.dll: bird.obj somb.lnk ../som_test/som.lnk
	$(WLINK) name $@ system os2v2_dll option quiet @../som_test/som.lnk @somb.lnk file bird.obj \
		export SOMInitModule,BirdNewClass,BirdClassData,BirdCClassData

clean:
	rm -f *.exe *.dll *.obj *.err

.PHONY: all clean
