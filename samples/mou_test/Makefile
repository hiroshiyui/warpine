# samples/mou_test/Makefile — MOUCALLS mouse API for a VIO program
WATCOM_ROOT = $(CURDIR)/../../vendor/watcom
WCL386 = $(WATCOM_ROOT)/binl64/wcl386
# 4OS2's bsesub.h declares the Mou* calls _Pascal instead of APIENTRY16, so
# they are direct 32-bit imports from MOUCALLS (no 16-bit thunks).
INCLUDE = $(CURDIR)/../4os2/h:$(WATCOM_ROOT)/h/os2

all: mou_test.exe

mou_test.exe: mou_test.c
	@if [ ! -d "$(WATCOM_ROOT)" ]; then \
		echo "Error: Open Watcom not found. Please run ../../vendor/setup_watcom.sh first."; \
		exit 1; \
	fi
	INCLUDE=$(INCLUDE) WATCOM=$(WATCOM_ROOT) PATH=$(WATCOM_ROOT)/binl64:$(PATH) \
	$(WCL386) -q -bt=os2 -l=os2v2 mou_test.c

clean:
	rm -f *.exe *.obj *.err
