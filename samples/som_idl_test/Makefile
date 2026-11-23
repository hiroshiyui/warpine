# SPDX-License-Identifier: GPL-3.0-or-later
# samples/som_idl_test/Makefile - classes declared in SOM IDL, bound by
# somidl's C emitters and built by GCC or clang (ADR-0122).
# `make` (CC=gcc) or `make CC=clang`; BUILD is where everything goes.
# Build the tools first: cargo build --bin lx_link --bin somidl.
include ../../sdk/os2.mk

BUILD  ?= .
CFLAGS  = $(OS2_CFLAGS) -std=gnu11 -Wall -Wextra -I$(BUILD)

# Open Watcom's SOM samples, their C sources unchanged, when its tree is
# there (vendor/setup_watcom.sh): built from bindings somidl generates.
OWSOM := $(wildcard $(WARPINE_SDK)/../vendor/watcom/samples/os2/som)
OWEXE := $(if $(OWSOM),$(BUILD)/ow_hello.exe $(BUILD)/ow_animals.exe)

ifeq ($(and $(wildcard $(LX_LINK)),$(wildcard $(SOMIDL))),)
all:
	@echo "som_idl_test: skipped, lx-link or somidl not built (cargo build --bin lx_link --bin somidl)"
else
all: $(BUILD)/som_idl_test.exe $(OWEXE)
endif

# The bindings: counter.h (usage) and counter.ih (implementation).
$(BUILD)/counter.h $(BUILD)/counter.ih &: counter.idl $(SOMIDL) $(wildcard $(WARPINE_SDK)/include/*.idl)
	$(SOMIDL) -I $(WARPINE_SDK)/include -s h,ih -o $(BUILD) counter.idl

$(BUILD)/counter.o: counter.c $(BUILD)/counter.ih $(wildcard $(WARPINE_SDK)/include/*.h)
	$(CC) $(CFLAGS) -c $< -o $@
$(BUILD)/main.o: main.c $(BUILD)/counter.h $(wildcard $(WARPINE_SDK)/include/*.h)
	$(CC) $(CFLAGS) -c $< -o $@

$(BUILD)/som_idl_test.exe: $(BUILD)/counter.o $(BUILD)/main.o $(OS2_CRT0) $(OS2_LIBC)
	$(LX_LINK) --gc-sections $(OS2_CRT0) $(BUILD)/counter.o $(BUILD)/main.o $(OS2_LIBC) -o $@

# hello and animals: their bindings into $(BUILD)/<name>/, their sources
# compiled as they are.
OWFLAGS = $(OS2_CFLAGS) -std=gnu11 -w
$(BUILD)/hello/hello.h $(BUILD)/hello/hello.ih &: $(OWSOM)/helloc/hello.idl $(SOMIDL)
	mkdir -p $(BUILD)/hello
	$(SOMIDL) -I $(WARPINE_SDK)/include -s h,ih -o $(BUILD)/hello $<
$(BUILD)/ow_hello_impl.o: $(OWSOM)/helloc/hello.c $(BUILD)/hello/hello.ih
	$(CC) $(OWFLAGS) -I$(BUILD)/hello -c $< -o $@
$(BUILD)/ow_hello_main.o: $(OWSOM)/helloc/main.c $(BUILD)/hello/hello.h
	$(CC) $(OWFLAGS) -I$(BUILD)/hello -c $< -o $@
$(BUILD)/animals/animals.h $(BUILD)/animals/animals.ih &: $(OWSOM)/animals/animals.idl $(SOMIDL)
	mkdir -p $(BUILD)/animals
	$(SOMIDL) -I $(WARPINE_SDK)/include -s h,ih -o $(BUILD)/animals $<
$(BUILD)/ow_animals_impl.o: $(OWSOM)/animals/animals.c $(BUILD)/animals/animals.ih
	$(CC) $(OWFLAGS) -I$(BUILD)/animals -c $< -o $@
$(BUILD)/ow_animals_main.o: $(OWSOM)/animals/main.c $(BUILD)/animals/animals.h
	$(CC) $(OWFLAGS) -I$(BUILD)/animals -c $< -o $@
$(BUILD)/ow_%.exe: $(BUILD)/ow_%_impl.o $(BUILD)/ow_%_main.o $(OS2_CRT0) $(OS2_LIBC)
	$(LX_LINK) --gc-sections $(OS2_CRT0) $(BUILD)/ow_$*_impl.o $(BUILD)/ow_$*_main.o $(OS2_LIBC) -o $@

clean:
	rm -f *.o *.exe counter.h counter.ih
	rm -rf hello animals

.PHONY: all clean
