# samples/ne_arena_test/Makefile — a 16-bit program cannot reach the shared arena (ADR-0049 addendum)
#
# Builds a minimal NE executable from pure assembly (ne_arena_test.asm),
# bypassing the Watcom C runtime entirely.  This avoids LDT-selector
# setup done by the CRT that our GDT-tile model cannot satisfy.
WATCOM_ROOT = $(CURDIR)/../../vendor/watcom
WASM = $(WATCOM_ROOT)/binl64/wasm
WLINK = $(WATCOM_ROOT)/binl64/wlink

all: ne_arena_test.exe

ne_arena_test.o: ne_arena_test.asm
	@if [ ! -d "$(WATCOM_ROOT)" ]; then \
		echo "Error: Open Watcom not found. Please run ../../vendor/setup_watcom.sh first."; \
		exit 1; \
	fi
	WATCOM=$(WATCOM_ROOT) $(WASM) -q -ms ne_arena_test.asm

ne_arena_test.exe: ne_arena_test.o ne_arena_test.lnk
	WATCOM=$(WATCOM_ROOT) $(WLINK) @ne_arena_test.lnk

clean:
	rm -f *.exe *.o *.obj *.err *.map
