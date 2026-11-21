# SPDX-License-Identifier: GPL-3.0-or-later
# samples/cxx_test/Makefile - a C++ program on Warpine's SDK (ADR-0103):
# `make` (g++) or `make CC=clang` (clang++); BUILD names where it goes.
include ../../sdk/os2.mk

BUILD ?= .
CXXFLAGS = $(OS2_CXXFLAGS) -Wall -Wextra

ifeq ($(wildcard $(LX_LINK)),)
all:
	@echo "cxx_test: skipped, lx-link not built (cargo build --bin lx_link)"
else
all: $(BUILD)/cxx_test.exe
endif

$(BUILD)/cxx_test.o: cxx_test.cc $(wildcard $(WARPINE_SDK)/include/c++/* $(WARPINE_SDK)/include/*.h)
	$(OS2_CXX) $(CXXFLAGS) -c $< -o $@

$(BUILD)/cxx_test.exe: $(BUILD)/cxx_test.o $(OS2_CRT0) $(OS2_LIBCXX) $(OS2_LIBC)
	$(LX_LINK) --gc-sections $(OS2_CRT0) $< $(OS2_LIBCXX) $(OS2_LIBC) -o $@

clean:
	rm -f *.o *.exe *.dbg

.PHONY: all clean
