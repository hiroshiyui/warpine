# samples/som_ordinal_test/Makefile
#
# SOM.DLL's IBM ordinals 1-47 (ADR-0070): a program that imports the SOM
# kernel by number (somord.lnk), as one linked against IBM's som.lib does.
# It uses the hand-written som.h of samples/som_test.

WATCOM_ROOT = $(CURDIR)/../../vendor/watcom
WCC386      = $(WATCOM_ROOT)/binl64/wcc386
WLINK       = $(WATCOM_ROOT)/binl64/wlink

export WATCOM  = $(WATCOM_ROOT)
export INCLUDE = $(WATCOM_ROOT)/h/os2:$(WATCOM_ROOT)/h
export PATH   := $(WATCOM_ROOT)/binl64:$(PATH)

all: somord.exe

somord.obj: somord.c ../som_test/include/som.h
	$(WCC386) -zq -bt=os2 -I$(CURDIR)/../som_test/include -fo=$@ $<

somord.exe: somord.obj somord.lnk
	$(WLINK) name $@ system os2v2 option quiet @somord.lnk file somord.obj

clean:
	rm -f *.exe *.obj *.err

.PHONY: all clean
