SHELL := bash
.SHELLFLAGS := -euo pipefail -c
.DEFAULT_GOAL := build

# ── Paths ─────────────────────────────────────────────────────────────────────

CAMDLC     := ocaml/_build/default/bin/camdlc.exe
CAMDL_SIM  := rust/target/release/camdl-sim
INSTALL_DIR := $(HOME)/.local/bin

OCAML_GOLDENS := $(wildcard ocaml/golden/*.camdl)

# ── Build ─────────────────────────────────────────────────────────────────────

.PHONY: build build-ocaml build-rust

build: build-ocaml build-rust

build-ocaml:
	cd ocaml && dune build

build-rust:
	cd rust && cargo build --release --workspace --bins

# ── Install ───────────────────────────────────────────────────────────────────

.PHONY: install uninstall

install: build
	@mkdir -p $(INSTALL_DIR)
	@# camdlc: dune uses .exe on all platforms; install without the suffix
	install -m 755 $(CAMDLC) $(INSTALL_DIR)/camdlc
	install -m 755 $(CAMDL_SIM) $(INSTALL_DIR)/camdl-sim
	install -m 755 bin/camdl $(INSTALL_DIR)/camdl
	@echo "Installed to $(INSTALL_DIR)"
	@echo "Make sure $(INSTALL_DIR) is on your PATH."

uninstall:
	rm -f $(INSTALL_DIR)/camdlc $(INSTALL_DIR)/camdl-sim $(INSTALL_DIR)/camdl
	@echo "Removed from $(INSTALL_DIR)"

# ── Test ──────────────────────────────────────────────────────────────────────

.PHONY: test test-ocaml test-rust

test: test-ocaml test-rust

test-ocaml:
	cd ocaml && dune runtest

test-rust:
	cd rust && cargo test --workspace

# ── Golden file management ────────────────────────────────────────────────────

.PHONY: update-golden update-ocaml-golden update-ir-golden

# Recompile all DSL fixtures → ocaml/golden/*.ir.json
update-ocaml-golden: build-ocaml
	@echo "Recompiling OCaml golden files..."
	@for src in $(OCAML_GOLDENS); do \
		out="$${src%.camdl}.ir.json"; \
		echo "  $$src → $$out"; \
		$(CAMDLC) "$$src" > "$$out"; \
	done

# Copy the shared ir/golden/ files from the compiled OCaml goldens
# (only the models that exist in both directories)
update-ir-golden: update-ocaml-golden
	@echo "Updating ir/golden from ocaml/golden..."
	@for f in seir_age sir_basic sir_demography; do \
		src="ocaml/golden/$$f.ir.json"; \
		dst="ir/golden/$$f.ir.json"; \
		if [ -f "$$src" ]; then \
			echo "  $$src → $$dst"; \
			cp "$$src" "$$dst"; \
		fi; \
	done

update-golden: update-ocaml-golden update-ir-golden

# ── Quick simulation helpers ──────────────────────────────────────────────────

.PHONY: sim

# Usage: make sim MODEL=ir/golden/sir_basic.ir.json ARGS="--set beta=0.3 ..."
sim: build-rust
	$(CAMDL_SIM) simulate $(MODEL) $(ARGS)

# ── Housekeeping ──────────────────────────────────────────────────────────────

.PHONY: clean

clean:
	cd ocaml && dune clean
	cd rust && cargo clean
