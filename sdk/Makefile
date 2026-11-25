# SPDX-License-Identifier: GPL-3.0-or-later
# sdk/Makefile - Warpine's small C runtime for GCC and clang (ADR-0096):
#   make            the runtime for gcc, in build/gcc/
#   make CC=clang   the runtime for clang, in build/clang/
# It writes crt0.o (link it first in a program), dll0.o (link it first in
# a DLL), libos2c.a (link it last), and libos2cxx.a, the C++ runtime
# (link it before libos2c.a; ADR-0103), with g++ or clang++; and, when
# somidl is built, libwps.a, the Workplace Shell classes (ADR-0129), with
# their bindings in include/ (`make wps` builds them alone).

WARPINE_SDK_BUILDING = 1
include os2.mk

LIBC_SRCS = string.c stdlib.c malloc.c stdio.c printf.c dtoa.c strtod.c math.c time.c scanf.c msgstub.c divdi3.c setjmp.c
LIBC_OBJS = $(LIBC_SRCS:%.c=$(OS2_BUILD)/%.o)
HEADERS   = $(wildcard include/*.h include/os2/*.h) libc/libc_internal.h

# The runtime's own memcpy and memset must not be turned into calls to
# themselves. OS2_CFLAGS's unwind tables let a C++ exception thrown for a
# trap in the C library (a bad pointer given to memcpy) unwind through it
# (ADR-0110).
LIBC_CFLAGS = $(OS2_CFLAGS) \
              -std=gnu11 -Wall -Wextra -Werror
ifeq ($(findstring clang,$(notdir $(CC))),)
LIBC_CFLAGS += -fno-tree-loop-distribute-patterns
endif

CXX_SRCS  = new.cc stdexcept.cc typeinfo.cc cxa_exception.cc
CXX_OBJS  = $(OS2_BUILD)/unwind.o $(CXX_SRCS:%.cc=$(OS2_BUILD)/%.o)
CXX_HEADERS = $(wildcard include/c++/*) include/unwind.h libcxx/cxxabi_internal.h

all: $(OS2_CRT0) $(OS2_DLL0) $(OS2_LIBC) $(OS2_LIBCXX)

$(OS2_BUILD)/%.o: libc/%.c $(HEADERS) | $(OS2_BUILD)
	$(CC) $(LIBC_CFLAGS) -c $< -o $@

$(OS2_LIBC): $(LIBC_OBJS)
	rm -f $@
	$(AR) rcs $@ $^

# The unwinder is C, with unwind tables of its own for the frames it
# starts from; the rest is C++.
$(OS2_BUILD)/unwind.o: libcxx/unwind.c $(HEADERS) include/unwind.h | $(OS2_BUILD)
	$(CC) $(LIBC_CFLAGS) -funwind-tables -c $< -o $@

$(OS2_BUILD)/%.o: libcxx/%.cc $(HEADERS) $(CXX_HEADERS) | $(OS2_BUILD)
	$(OS2_CXX) $(OS2_CXXFLAGS) -Wall -Wextra -Werror -c $< -o $@

$(OS2_LIBCXX): $(CXX_OBJS)
	rm -f $@
	$(AR) rcs $@ $^

# The Workplace Shell classes: each one's IDL in include/, its C in wps/;
# usage bindings into $(OS2_BUILD)/include, implementation bindings into
# $(OS2_BUILD)/wps, then the library.
WPS_CLASSES = wpobject wpabstract wptrans wpfsys wpdataf
WPS_IDLS    = $(WPS_CLASSES:%=include/%.idl) include/wptypes.idl $(wildcard include/som*.idl)
WPS_HEADERS = $(WPS_CLASSES:%=$(OS2_BUILD)/include/%.h)
WPS_CFLAGS  = $(OS2_CFLAGS) -std=gnu11 -Wall -Wextra -Werror -I$(OS2_BUILD)/wps
ifneq ($(wildcard $(SOMIDL)),)
all: $(OS2_LIBWPS)
endif
wps: $(OS2_LIBWPS)

$(OS2_BUILD)/include/%.h $(OS2_BUILD)/wps/%.ih &: include/%.idl $(WPS_IDLS) $(SOMIDL)
	mkdir -p $(OS2_BUILD)/include $(OS2_BUILD)/wps
	$(SOMIDL) -I include -s h -o $(OS2_BUILD)/include $<
	$(SOMIDL) -I include -s ih -o $(OS2_BUILD)/wps $<

$(OS2_BUILD)/wps/%.o: wps/%.c $(OS2_BUILD)/wps/%.ih $(WPS_HEADERS) wps/wpsint.h $(HEADERS)
	$(CC) $(WPS_CFLAGS) -c $< -o $@

$(OS2_LIBWPS): $(WPS_CLASSES:%=$(OS2_BUILD)/wps/%.o)
	rm -f $@
	$(AR) rcs $@ $^

$(OS2_BUILD):
	mkdir -p $@

clean:
	rm -rf build

.PHONY: all clean wps
