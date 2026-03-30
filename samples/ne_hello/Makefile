# samples/ne_hello/Makefile — 16-bit OS/2 NE format hello world
#
# Builds a minimal NE executable from pure assembly (ne_hello.asm),
# bypassing the Watcom C runtime entirely.  This avoids LDT-selector
# setup done by the CRT that our GDT-tile model cannot satisfy.
WATCOM_ROOT = $(CURDIR)/../../vendor/watcom
WASM = $(WATCOM_ROOT)/binl64/wasm
WLINK = $(WATCOM_ROOT)/binl64/wlink

all: ne_hello.exe

ne_hello.o: ne_hello.asm
	@if [ ! -d "$(WATCOM_ROOT)" ]; then \
		echo "Error: Open Watcom not found. Please run ../../vendor/setup_watcom.sh first."; \
		exit 1; \
	fi
	WATCOM=$(WATCOM_ROOT) $(WASM) -q -ms ne_hello.asm

ne_hello.exe: ne_hello.o ne_hello.lnk
	WATCOM=$(WATCOM_ROOT) $(WLINK) @ne_hello.lnk

clean:
	rm -f *.exe *.o *.obj *.err *.map
