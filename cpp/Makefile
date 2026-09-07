# Two compilers, because the point of this port is to be the reference an
# ordinary C++ programmer would write and check against.
CXX      ?= g++
CXXFLAGS ?= -std=c++20 -O2 -march=native -Wall -Wextra -Wpedantic -Werror -pthread
SRC       = src/csvdiff.cpp src/json.cpp src/main.cpp
BIN       = build/csvdiff
GEN       = build/gen-data

all: $(BIN)

$(BIN): $(SRC) src/csvdiff.hpp
	@mkdir -p build
	$(CXX) $(CXXFLAGS) -o $@ $(SRC)

# The benchmark data generator. Writes the same bytes as the five other
# generators; it is here because it is the only one fast enough to keep up with
# the disk. See tools/gen_data.cpp.
gen-data: $(GEN)

$(GEN): tools/gen_data.cpp
	@mkdir -p build
	$(CXX) $(CXXFLAGS) -o $@ tools/gen_data.cpp

clean:
	rm -rf build

.PHONY: all gen-data clean
