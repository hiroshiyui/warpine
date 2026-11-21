# SPDX-License-Identifier: GPL-3.0-or-later
# samples/c_gdb_test/Makefile - a program built with debugging information
# (ADR-0102): `make` (CC=gcc) or `make CC=clang`; BUILD names where the
# program and its debug companion (c_gdb_test.dbg) go.
include ../../sdk/os2.mk

BUILD ?= .
# No optimisation, so every variable lives where the debugging
# information says.
CFLAGS = $(filter-out -O2,$(OS2_CFLAGS)) -O0 -g -std=gnu11 -Wall -Wextra

ifeq ($(wildcard $(LX_LINK)),)
all:
	@echo "c_gdb_test: skipped, lx-link not built (cargo build --bin lx_link)"
else
all: $(BUILD)/c_gdb_test.exe
endif

$(BUILD)/c_gdb_test.o: c_gdb_test.c $(wildcard $(WARPINE_SDK)/include/*.h $(WARPINE_SDK)/include/os2/*.h)
	$(CC) $(CFLAGS) -c $< -o $@

# lx-link writes $(BUILD)/c_gdb_test.dbg too, since the object has DWARF.
$(BUILD)/c_gdb_test.exe: $(BUILD)/c_gdb_test.o $(OS2_CRT0) $(OS2_LIBC)
	$(LX_LINK) --gc-sections $(OS2_CRT0) $< $(OS2_LIBC) -o $@

clean:
	rm -f *.o *.exe *.dbg

.PHONY: all clean
