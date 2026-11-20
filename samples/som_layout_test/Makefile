# samples/som_layout_test/Makefile
#
# IBM's SOM layouts (ADR-0072): ClassData in IBM's release order, the
# somMethodTab header, CClassData.parentMtab as a somParentMtabStruct, and
# the SOMClass methods and kernel entries that come with them. Built
# against the hand-written som.h of samples/som_test, which follows IBM's
# release order.

WATCOM_ROOT = $(CURDIR)/../../vendor/watcom
WCC386      = $(WATCOM_ROOT)/binl64/wcc386
WLINK       = $(WATCOM_ROOT)/binl64/wlink

export WATCOM  = $(WATCOM_ROOT)
export INCLUDE = $(WATCOM_ROOT)/h/os2:$(WATCOM_ROOT)/h
export PATH   := $(WATCOM_ROOT)/binl64:$(PATH)

all: somlay.exe

somlay.obj: somlay.c ../som_test/include/som.h
	$(WCC386) -zq -bt=os2 -I$(CURDIR)/../som_test/include -fo=$@ $<

somlay.exe: somlay.obj somlay.lnk ../som_test/som.lnk
	$(WLINK) name $@ system os2v2 option quiet @../som_test/som.lnk @somlay.lnk file somlay.obj

clean:
	rm -f *.exe *.obj *.err

.PHONY: all clean
