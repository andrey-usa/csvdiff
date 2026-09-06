# Two compilers, because the point of this port is to be the reference an
# ordinary C++ programmer would write and check against.
CXX      ?= g++
CXXFLAGS ?= -std=c++20 -O2 -march=native -Wall -Wextra -Wpedantic -Werror
SRC       = src/csvdiff.cpp src/json.cpp src/main.cpp
BIN       = build/csvdiff

all: $(BIN)

$(BIN): $(SRC) src/csvdiff.hpp
	@mkdir -p build
	$(CXX) $(CXXFLAGS) -o $@ $(SRC)

clean:
	rm -rf build

.PHONY: all clean
