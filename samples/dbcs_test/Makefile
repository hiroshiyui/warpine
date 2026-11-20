# samples/dbcs_test/Makefile
# Built against bsesub32.mk's copy of bsesub.h, which declares the Vio*,
# Kbd* and Mou* calls _Pascal instead of APIENTRY16: direct 32-bit imports.
WATCOM_ROOT = $(CURDIR)/../../vendor/watcom
include ../bsesub32.mk
WCC386      = $(WATCOM_ROOT)/binl64/wcc386
WLINK       = $(WATCOM_ROOT)/binl64/wlink
INCLUDE     = $(BSESUB32_DIR):$(WATCOM_ROOT)/h/os2:$(WATCOM_ROOT)/h

all: dbcs_test.exe

dbcs_test.exe: dbcs_test.obj
	INCLUDE=$(INCLUDE) WATCOM=$(WATCOM_ROOT) PATH=$(WATCOM_ROOT)/binl64:$(PATH) \
	$(WLINK) system os2v2 \
	    file dbcs_test.obj \
	    name dbcs_test.exe

dbcs_test.obj: dbcs_test.c $(BSESUB32_H)
	@if [ ! -d "$(WATCOM_ROOT)" ]; then \
		echo "Error: Open Watcom not found. Run ../../vendor/setup_watcom.sh first."; \
		exit 1; \
	fi
	INCLUDE=$(INCLUDE) WATCOM=$(WATCOM_ROOT) PATH=$(WATCOM_ROOT)/binl64:$(PATH) \
	$(WCC386) -q -bt=os2 -fo=dbcs_test.obj dbcs_test.c

clean:
	rm -rf build-bsesub32
	rm -f *.exe *.obj *.err *.map
