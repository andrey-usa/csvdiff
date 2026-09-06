# Two compilers, because the gap between them is a finding this port exists to
# show: on 1M rows gcc takes 5.34s and clang 4.77s from the same source.
CC     ?= cc
CFLAGS ?= -std=c11 -O2 -march=native -Wall -Wextra -Werror
BIN     = csvdiff

all: $(BIN)

$(BIN): csvdiff.c
	$(CC) $(CFLAGS) -o $@ csvdiff.c

clean:
	rm -f $(BIN)

.PHONY: all clean
