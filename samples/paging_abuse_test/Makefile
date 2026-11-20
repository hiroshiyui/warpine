# SPDX-License-Identifier: GPL-3.0-or-later
# samples/paging_abuse_test/Makefile
WATCOM_ROOT = $(CURDIR)/../../vendor/watcom
WCL386      = $(WATCOM_ROOT)/binl64/wcl386
INCLUDE     = $(WATCOM_ROOT)/h/os2;$(WATCOM_ROOT)/h
WCLENV      = INCLUDE="$(INCLUDE)" WATCOM=$(WATCOM_ROOT) PATH=$(WATCOM_ROOT)/binl64:$(PATH)

all: pgabuse_zero.exe pgabuse_cr3.exe pgabuse_remap.exe pgabuse_off.exe pgabuse_past.exe

pgabuse_zero.exe: paging_abuse_test.c
	$(WCLENV) $(WCL386) -q -bt=os2 -l=os2v2 -dZERO -fo=pgabuse_zero.obj -fe=$@ paging_abuse_test.c

pgabuse_cr3.exe: paging_abuse_test.c
	$(WCLENV) $(WCL386) -q -bt=os2 -l=os2v2 -dCR3 -fo=pgabuse_cr3.obj -fe=$@ paging_abuse_test.c

pgabuse_remap.exe: paging_abuse_test.c
	$(WCLENV) $(WCL386) -q -bt=os2 -l=os2v2 -dREMAP -fo=pgabuse_remap.obj -fe=$@ paging_abuse_test.c

pgabuse_off.exe: paging_abuse_test.c
	$(WCLENV) $(WCL386) -q -bt=os2 -l=os2v2 -dPAGING_OFF -fo=pgabuse_off.obj -fe=$@ paging_abuse_test.c

pgabuse_past.exe: paging_abuse_test.c
	$(WCLENV) $(WCL386) -q -bt=os2 -l=os2v2 -dPAST -fo=pgabuse_past.obj -fe=$@ paging_abuse_test.c

clean:
	rm -f *.exe *.obj *.o *.err
