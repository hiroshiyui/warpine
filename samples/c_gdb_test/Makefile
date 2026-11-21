# SPDX-License-Identifier: GPL-3.0-or-later
# samples/c_gdb_test/Makefile - a program and two DLLs built with debugging
# information (ADR-0102, ADR-0104): `make` (CC=gcc) or `make CC=clang`;
# BUILD names where they and their debug companions (*.dbg) go.
include ../../sdk/os2.mk

BUILD ?= .
# No optimisation, so every variable lives where the debugging
# information says.
CFLAGS = $(filter-out -O2,$(OS2_CFLAGS)) -O0 -g -std=gnu11 -Wall -Wextra

ifeq ($(wildcard $(LX_LINK)),)
all:
	@echo "c_gdb_test: skipped, lx-link not built (cargo build --bin lx_link)"
else
all: $(BUILD)/c_gdb_test.exe $(BUILD)/gdbdyn.dll
endif

$(BUILD)/%.o: %.c gdb_point.h $(wildcard $(WARPINE_SDK)/include/*.h $(WARPINE_SDK)/include/os2/*.h)
	$(CC) $(CFLAGS) -c $< -o $@

# lx-link writes a .dbg beside each module, since the objects have DWARF.
$(BUILD)/gdbdll.dll: $(BUILD)/gdbdll.o $(OS2_DLL0) $(OS2_LIBC)
	$(LX_LINK) --dll --export scale_point --initinstance --terminstance $(OS2_DLL0) $< $(OS2_LIBC) -o $@

$(BUILD)/gdbdyn.dll: $(BUILD)/gdbdyn.o $(OS2_DLL0) $(OS2_LIBC)
	$(LX_LINK) --dll --export dyn_twice --initinstance --terminstance $(OS2_DLL0) $< $(OS2_LIBC) -o $@

# The program imports scale_point from gdbdll.dll.
$(BUILD)/c_gdb_test.exe: $(BUILD)/c_gdb_test.o $(BUILD)/gdbdll.dll $(OS2_CRT0) $(OS2_LIBC)
	$(LX_LINK) --gc-sections $(OS2_CRT0) $(BUILD)/c_gdb_test.o $(BUILD)/gdbdll.dll $(OS2_LIBC) -o $@

clean:
	rm -f *.o *.exe *.dll *.dbg

.PHONY: all clean
