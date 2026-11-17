# samples/Makefile — build every Open Watcom sample in one go.
#
#   ./vendor/setup_watcom.sh      # once, from the repository root
#   make -C samples               # build all Watcom samples
#   make -C samples clean
#
# Each sample keeps its own Makefile; this one only visits them. Left out:
#   4os2       needs its source first: cd samples/4os2 && ./fetch_source.sh && make
#   rust_hello a Rust guest built with cargo +nightly (see its README section)

SKIP     := 4os2
SAMPLES  := $(filter-out $(SKIP),$(patsubst %/Makefile,%,$(wildcard */Makefile)))

.PHONY: all clean $(SAMPLES)

all: $(SAMPLES)

$(SAMPLES):
	$(MAKE) -C $@

clean:
	@for d in $(SAMPLES); do $(MAKE) -C $$d clean || exit 1; done
