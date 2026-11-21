# SPDX-License-Identifier: GPL-3.0-or-later
# samples/cxx_test/Makefile - a C++ program and a C++ DLL on Warpine's SDK
# (ADR-0103, ADR-0107): `make` (g++) or `make CC=clang` (clang++); BUILD
# names where they go.
include ../../sdk/os2.mk

BUILD ?= .
CXXFLAGS = $(OS2_CXXFLAGS) -Wall -Wextra

ifeq ($(wildcard $(LX_LINK)),)
all:
	@echo "cxx_test: skipped, lx-link not built (cargo build --bin lx_link)"
else
all: $(BUILD)/cxx_test.exe
endif

$(BUILD)/%.o: %.cc cxxdll.h $(wildcard $(WARPINE_SDK)/include/c++/* $(WARPINE_SDK)/include/*.h)
	$(OS2_CXX) $(CXXFLAGS) -c $< -o $@

# The DLL links its own copy of the C++ and C runtimes.
$(BUILD)/cxxdll.dll: $(BUILD)/cxxdll.o $(OS2_DLL0) $(OS2_LIBCXX) $(OS2_LIBC)
	$(LX_LINK) --gc-sections --dll --export dll_throw,dll_catch,dll_pass,dll_rethrow,dll_uncaught \
	    --initinstance --terminstance $(OS2_DLL0) $< $(OS2_LIBCXX) $(OS2_LIBC) -o $@

$(BUILD)/cxx_test.exe: $(BUILD)/cxx_test.o $(BUILD)/cxxdll.dll $(OS2_CRT0) $(OS2_LIBCXX) $(OS2_LIBC)
	$(LX_LINK) --gc-sections $(OS2_CRT0) $< $(BUILD)/cxxdll.dll $(OS2_LIBCXX) $(OS2_LIBC) -o $@

clean:
	rm -f *.o *.exe *.dll *.dbg

.PHONY: all clean
