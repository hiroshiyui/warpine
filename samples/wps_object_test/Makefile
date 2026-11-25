# SPDX-License-Identifier: GPL-3.0-or-later
# samples/wps_object_test/Makefile - a Workplace object class of the
# program's own over the SDK's WPObject (libwps, ADR-0129), bound by
# somidl and built by GCC or clang.
# `make` (CC=gcc) or `make CC=clang`; BUILD is where everything goes.
# Build the tools first: cargo build --bin lx_link --bin somidl.
include ../../sdk/os2.mk

BUILD  ?= build
CFLAGS  = $(OS2_CFLAGS) -std=gnu11 -Wall -Wextra -I$(BUILD)

ifeq ($(and $(wildcard $(LX_LINK)),$(wildcard $(SOMIDL))),)
all:
	@echo "wps_object_test: skipped, lx-link or somidl not built (cargo build --bin lx_link --bin somidl)"
else
all: $(BUILD)/wps_object_test.exe
endif

# libwps first: it generates <wpobject.h>, which note.h includes.
$(BUILD)/note.h $(BUILD)/note.ih &: note.idl $(SOMIDL) $(OS2_LIBWPS) $(wildcard $(WARPINE_SDK)/include/*.idl)
	mkdir -p $(BUILD)
	$(SOMIDL) -I $(WARPINE_SDK)/include -s h,ih -o $(BUILD) note.idl

$(BUILD)/note.o: note.c $(BUILD)/note.ih
	$(CC) $(CFLAGS) -c $< -o $@
$(BUILD)/main.o: main.c $(BUILD)/note.h
	$(CC) $(CFLAGS) -c $< -o $@

$(BUILD)/wps_object_test.exe: $(BUILD)/note.o $(BUILD)/main.o $(OS2_CRT0) $(OS2_LIBWPS) $(OS2_LIBC)
	$(LX_LINK) --gc-sections $(OS2_CRT0) $(BUILD)/note.o $(BUILD)/main.o $(OS2_LIBWPS) $(OS2_LIBC) -o $@

clean:
	rm -rf build

.PHONY: all clean
