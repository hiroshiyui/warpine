# samples/dbcs_test/Makefile
# Built against 4OS2's bsesub.h, which declares the Vio* calls _Pascal
# instead of APIENTRY16, so they are direct 32-bit imports from VIOCALLS
# (VioWrtCellStr 10, VIO16CHECKCHARTYPE 75 from os2386.lib).
WATCOM_ROOT = $(CURDIR)/../../vendor/watcom
WCC386      = $(WATCOM_ROOT)/binl64/wcc386
WLINK       = $(WATCOM_ROOT)/binl64/wlink
INCLUDE     = $(CURDIR)/../4os2/h:$(WATCOM_ROOT)/h/os2:$(WATCOM_ROOT)/h

all: dbcs_test.exe

dbcs_test.exe: dbcs_test.obj
	INCLUDE=$(INCLUDE) WATCOM=$(WATCOM_ROOT) PATH=$(WATCOM_ROOT)/binl64:$(PATH) \
	$(WLINK) system os2v2 \
	    file dbcs_test.obj \
	    name dbcs_test.exe

dbcs_test.obj: dbcs_test.c
	@if [ ! -d "$(WATCOM_ROOT)" ]; then \
		echo "Error: Open Watcom not found. Run ../../vendor/setup_watcom.sh first."; \
		exit 1; \
	fi
	INCLUDE=$(INCLUDE) WATCOM=$(WATCOM_ROOT) PATH=$(WATCOM_ROOT)/binl64:$(PATH) \
	$(WCC386) -q -bt=os2 -fo=dbcs_test.obj dbcs_test.c

clean:
	rm -f *.exe *.obj *.err *.map
