# samples/vio_kbd_test/Makefile — VIO text and KBD calls test
WATCOM_ROOT = $(CURDIR)/../../vendor/watcom
include ../bsesub32.mk
WCL386 = $(WATCOM_ROOT)/binl64/wcl386
# Built against bsesub32.mk's copy of bsesub.h, which declares the Vio*,
# Kbd* and Mou* calls _Pascal instead of APIENTRY16: direct 32-bit imports.
INCLUDE = $(BSESUB32_DIR):$(WATCOM_ROOT)/h/os2

all: vio_kbd_test.exe

vio_kbd_test.exe: vio_kbd_test.c $(BSESUB32_H)
	@if [ ! -d "$(WATCOM_ROOT)" ]; then \
		echo "Error: Open Watcom not found. Please run ../../vendor/setup_watcom.sh first."; \
		exit 1; \
	fi
	INCLUDE=$(INCLUDE) WATCOM=$(WATCOM_ROOT) PATH=$(WATCOM_ROOT)/binl64:$(PATH) \
	$(WCL386) -q -bt=os2 -l=os2v2 -fm=vio_kbd_test.map vio_kbd_test.c

clean:
	rm -rf build-bsesub32
	rm -f *.exe *.obj *.err
