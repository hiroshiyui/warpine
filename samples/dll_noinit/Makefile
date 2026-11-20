# SPDX-License-Identifier: GPL-3.0-or-later
# samples/dll_noinit/Makefile
#
# noinit.dll: three exports and no initialization routine (no C runtime,
# no -bd), linked as a plain LX DLL. NoInitAdd (1) and NoInitTwice (2) are
# in the non-resident names table, NoInitAnswer (3) in the resident one.

WATCOM_ROOT = $(CURDIR)/../../vendor/watcom
WCC386      = $(WATCOM_ROOT)/binl64/wcc386
WLINK       = $(WATCOM_ROOT)/binl64/wlink

export WATCOM  = $(WATCOM_ROOT)
export INCLUDE = $(WATCOM_ROOT)/h/os2:$(WATCOM_ROOT)/h
export PATH   := $(WATCOM_ROOT)/binl64:$(PATH)

all: noinit.dll

noinit.obj: noinit.c
	$(WCC386) -zq -bt=os2 -zl -s -fo=$@ $<

noinit.dll: noinit.obj
	$(WLINK) format os2 lx dll name $@ option quiet option nodefaultlibs file $< \
		export NoInitAdd.1,NoInitTwice.2,NoInitAnswer.3 resident

clean:
	rm -f *.dll *.obj *.err

.PHONY: all clean
