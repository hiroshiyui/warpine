# samples/Makefile — build every Open Watcom sample in one go.
#
#   ./vendor/setup_watcom.sh      # once, from the repository root
#   make -C samples               # build all Watcom samples
#   make -C samples clean
#
# Each sample keeps its own Makefile; this one only visits them. Left out:
#   rust_hello a Rust guest built with cargo +nightly (see its README section)
# c_hello and c_sdk_test are built with GCC (or clang) and lx-link, not
# Open Watcom; they are skipped until lx-link is built (cargo build --bin lx_link).

SAMPLES  := $(patsubst %/Makefile,%,$(wildcard */Makefile))

.PHONY: all clean $(SAMPLES)

all: $(SAMPLES)

$(SAMPLES):
	$(MAKE) -C $@

clean:
	@for d in $(SAMPLES); do $(MAKE) -C $$d clean || exit 1; done
