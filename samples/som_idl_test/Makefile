# SPDX-License-Identifier: GPL-3.0-or-later
# samples/som_idl_test/Makefile - classes declared in SOM IDL, bound by
# somidl's C emitters and built by GCC or clang (ADR-0122).
# `make` (CC=gcc) or `make CC=clang`; BUILD is where everything goes.
# Build the tools first: cargo build --bin lx_link --bin somidl.
include ../../sdk/os2.mk

BUILD  ?= .
SOMIDL ?= $(WARPINE_SDK)/../target/debug/somidl
CFLAGS  = $(OS2_CFLAGS) -std=gnu11 -Wall -Wextra -I$(BUILD)

ifeq ($(and $(wildcard $(LX_LINK)),$(wildcard $(SOMIDL))),)
all:
	@echo "som_idl_test: skipped, lx-link or somidl not built (cargo build --bin lx_link --bin somidl)"
else
all: $(BUILD)/som_idl_test.exe
endif

# The bindings: counter.h (usage) and counter.ih (implementation).
$(BUILD)/counter.h $(BUILD)/counter.ih &: counter.idl $(wildcard $(WARPINE_SDK)/include/*.idl)
	$(SOMIDL) -I $(WARPINE_SDK)/include -s h,ih -o $(BUILD) counter.idl

$(BUILD)/counter.o: counter.c $(BUILD)/counter.ih $(wildcard $(WARPINE_SDK)/include/*.h)
	$(CC) $(CFLAGS) -c $< -o $@
$(BUILD)/main.o: main.c $(BUILD)/counter.h $(wildcard $(WARPINE_SDK)/include/*.h)
	$(CC) $(CFLAGS) -c $< -o $@

$(BUILD)/som_idl_test.exe: $(BUILD)/counter.o $(BUILD)/main.o $(OS2_CRT0) $(OS2_LIBC)
	$(LX_LINK) --gc-sections $(OS2_CRT0) $(BUILD)/counter.o $(BUILD)/main.o $(OS2_LIBC) -o $@

clean:
	rm -f *.o *.exe counter.h counter.ih

.PHONY: all clean
