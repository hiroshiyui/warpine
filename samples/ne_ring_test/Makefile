# samples/ne_ring_test/Makefile — the ring a 16-bit OS/2 program runs at (ADR-0113)
#
# Builds a minimal NE executable from pure assembly (ne_ring_test.asm),
# bypassing the Watcom C runtime entirely.  This avoids LDT-selector
# setup done by the CRT that our GDT-tile model cannot satisfy.
WATCOM_ROOT = $(CURDIR)/../../vendor/watcom
WASM = $(WATCOM_ROOT)/binl64/wasm
WLINK = $(WATCOM_ROOT)/binl64/wlink

all: ne_ring_test.exe

ne_ring_test.o: ne_ring_test.asm
	@if [ ! -d "$(WATCOM_ROOT)" ]; then \
		echo "Error: Open Watcom not found. Please run ../../vendor/setup_watcom.sh first."; \
		exit 1; \
	fi
	WATCOM=$(WATCOM_ROOT) $(WASM) -q -ms ne_ring_test.asm

ne_ring_test.exe: ne_ring_test.o ne_ring_test.lnk
	WATCOM=$(WATCOM_ROOT) $(WLINK) @ne_ring_test.lnk

clean:
	rm -f *.exe *.o *.obj *.err *.map
