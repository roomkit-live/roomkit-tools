CARGO ?= cargo
MSRV := $(shell sed -n 's/^rust-version = "\(.*\)"$$/\1/p' Cargo.toml).0
EXAMPLES := $(sort $(basename $(notdir $(wildcard examples/*.rs))))
DENY_FLAGS ?=
ROOMKIT_MANIFEST := adapters/roomkit/Cargo.toml
PYTHON_MANIFEST := bindings/python/Cargo.toml
PYTHON ?= python3
TARGET_DIR := $(or $(CARGO_TARGET_DIR),target)
ADAPTER_TARGET_DIR := $(or $(CARGO_TARGET_DIR),adapters/roomkit/target)
API_REPORT := $(TARGET_DIR)/api-parity
HIDDEN_TARGET_DIR := $(TARGET_DIR)/api-hidden
ADAPTER_SURFACE := api/roomkit-adapter-surface.json
API_PARITY_FLAGS ?=

.PHONY: all fmt fmt-check clippy test doc deny check-core-deps examples msrv msrv-toolchain hooks roomkit-check roomkit-msrv python-check python-msrv api-snapshot api-test api-hidden api-parity api-adapter-surface api-adapter-check

all: fmt-check clippy test doc deny check-core-deps examples api-test api-parity

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all -- --check

clippy:
	$(CARGO) clippy --workspace --all-targets --all-features --locked -- -D warnings

test:
	$(CARGO) test --workspace --all-features --locked

doc:
	RUSTDOCFLAGS="-D warnings" $(CARGO) doc --workspace --no-deps --all-features --locked

deny:
	@command -v cargo-deny >/dev/null 2>&1 || { \
		echo "Install cargo-deny: cargo install cargo-deny --locked" >&2; exit 1; \
	}
	$(CARGO) deny $(DENY_FLAGS) check

check-core-deps:
	CARGO="$(CARGO)" bash scripts/check-core-deps.sh

examples:
	$(CARGO) build --examples --locked --package roomkit-tools-examples
	@for example in $(EXAMPLES); do \
		$(CARGO) run --quiet --locked --package roomkit-tools-examples --example "$$example" || exit 1; \
	done

msrv:
	$(CARGO) +$(MSRV) check --workspace --all-targets --all-features --locked

msrv-toolchain:
	rustup toolchain install $(MSRV) --profile minimal

hooks:
	git config core.hooksPath .githooks

# Opt in with Git read access to the pinned roomkit-rs revision.
roomkit-check:
	$(CARGO) fmt --manifest-path $(ROOMKIT_MANIFEST) --all -- --check
	$(CARGO) clippy --manifest-path $(ROOMKIT_MANIFEST) --all-targets --all-features --locked -- -D warnings
	$(CARGO) test --manifest-path $(ROOMKIT_MANIFEST) --all-features --locked
	RUSTDOCFLAGS="-D warnings" $(CARGO) doc --manifest-path $(ROOMKIT_MANIFEST) --no-deps --all-features --locked
	$(MAKE) --no-print-directory api-adapter-check

roomkit-msrv:
	$(CARGO) +$(MSRV) check --manifest-path $(ROOMKIT_MANIFEST) --all-targets --all-features --locked

# One public API in Python and in Rust (CONTRIBUTING.md). The snapshots of the
# Python packages are rewritten from freshly built wheels, and the Python tests
# fail while they differ from the code.
api-snapshot:
	$(PYTHON) scripts/api-snapshot.py

# The tests of the rustdoc reader and of the parity gate, on fixtures.
api-test:
	PYTHONDONTWRITEBYTECODE=1 $(PYTHON) -m unittest discover -s scripts/tests

# The JSON rustdoc writes with --document-hidden-items, from which the gate
# lists the public items #[doc(hidden)] hides. Both options are unstable:
# RUSTC_BOOTSTRAP=1 lets the pinned rustdoc take them for this build only, in
# a target directory of its own, which leaves the HTML documentation alone.
api-hidden:
	RUSTC_BOOTSTRAP=1 RUSTDOCFLAGS="-Z unstable-options --output-format json --document-hidden-items" \
		$(CARGO) doc --workspace --no-deps --all-features --locked --target-dir $(HIDDEN_TARGET_DIR)

# The parity gate: the crates' public surface read from rustdoc's HTML, with the
# RoomKit adapter's committed surface, against the Python snapshots through
# api/mapping.toml, and the defaults of the structs standing for Python's
# keywords, which a test dumps since rustdoc shows none. The public items
# #[doc(hidden)] hides are each listed in the mapping with their decision. It
# fails on a difference no entry covers; pass API_PARITY_FLAGS=--strict to fail
# on a pending one too. The report, the surface, the hidden items, the defaults
# and the findings as JSON stay in $(API_REPORT).
api-parity: doc api-hidden
	@mkdir -p $(API_REPORT)
	@rm -f $(API_REPORT)/rs-defaults.json
	ROOMKIT_API_DEFAULTS=$(abspath $(API_REPORT))/rs-defaults.json $(CARGO) test --quiet --locked --all-features \
		--package roomkit-tools-examples --test api_defaults
	PYTHONDONTWRITEBYTECODE=1 $(PYTHON) scripts/rs-surface.py --doc $(TARGET_DIR)/doc > $(API_REPORT)/rs-surface.json
	PYTHONDONTWRITEBYTECODE=1 $(PYTHON) scripts/rs-surface.py --hidden --doc $(HIDDEN_TARGET_DIR)/doc > $(API_REPORT)/rs-hidden.json
	PYTHONDONTWRITEBYTECODE=1 $(PYTHON) scripts/api-parity.py $(API_REPORT)/rs-surface.json $(ADAPTER_SURFACE) \
		--defaults $(API_REPORT)/rs-defaults.json --hidden $(API_REPORT)/rs-hidden.json \
		$(API_PARITY_FLAGS) --json $(API_REPORT)/report.json

# The RoomKit adapter's surface is committed: its engine source is not public.
# Rewrite it after changing the adapter; roomkit-check checks it is current.
api-adapter-surface:
	RUSTDOCFLAGS="-D warnings" $(CARGO) doc --manifest-path $(ROOMKIT_MANIFEST) --no-deps --all-features --locked
	@mkdir -p $(API_REPORT)
	PYTHONDONTWRITEBYTECODE=1 $(PYTHON) scripts/rs-surface.py --doc $(ADAPTER_TARGET_DIR)/doc --crate roomkit_tools_roomkit > $(API_REPORT)/adapter-surface.json
	cp $(API_REPORT)/adapter-surface.json $(ADAPTER_SURFACE)

api-adapter-check:
	@mkdir -p $(API_REPORT)
	PYTHONDONTWRITEBYTECODE=1 $(PYTHON) scripts/rs-surface.py --doc $(ADAPTER_TARGET_DIR)/doc --crate roomkit_tools_roomkit > $(API_REPORT)/adapter-surface.json
	@cmp -s $(API_REPORT)/adapter-surface.json $(ADAPTER_SURFACE) || { \
		echo "$(ADAPTER_SURFACE) is not the adapter's current surface: run make api-adapter-surface" >&2; exit 1; \
	}

# Optional CPython bridge: the standalone Rust gates remain interpreter-free.
python-check:
	$(CARGO) fmt --manifest-path $(PYTHON_MANIFEST) --all -- --check
	$(CARGO) clippy --manifest-path $(PYTHON_MANIFEST) --all-targets --locked -- -D warnings
	RUSTDOCFLAGS="-D warnings" $(CARGO) doc --manifest-path $(PYTHON_MANIFEST) --no-deps --locked
	$(CARGO) deny --manifest-path $(PYTHON_MANIFEST) --config deny.toml check
	uv tool run --from 'ruff>=0.14,<1' ruff check python tests/python examples scripts/check-python.py scripts/native-notices.py scripts/artifact_files.py scripts/api-parity.py scripts/api-snapshot.py scripts/api_parity scripts/tests/test_api_parity.py scripts/tests/test_pre_push.py
	uv tool run --from 'ruff>=0.14,<1' ruff format --check python tests/python examples scripts/check-python.py scripts/native-notices.py scripts/artifact_files.py scripts/api-parity.py scripts/api-snapshot.py scripts/api_parity scripts/tests/test_api_parity.py scripts/tests/test_pre_push.py
	$(PYTHON) scripts/check-python.py

python-msrv:
	$(CARGO) +$(MSRV) check --manifest-path $(PYTHON_MANIFEST) --all-targets --locked

# Optional OCR engine distribution; set ROOMKIT_OCR_MODELS to run local model parity.
.PHONY: ocr-python-check ocr-python-msrv
ocr-python-check:
	$(CARGO) fmt --manifest-path backends/rusto/python/Cargo.toml --all -- --check
	$(CARGO) clippy --manifest-path backends/rusto/python/Cargo.toml --all-targets --locked -- -D warnings
	$(CARGO) deny --manifest-path backends/rusto/python/Cargo.toml --config deny.toml check
	uv tool run --from 'ruff>=0.14,<1' ruff check backends/rusto/python scripts/check-ocr-python.py scripts/native-notices.py scripts/artifact_files.py
	uv tool run --from 'ruff>=0.14,<1' ruff format --check backends/rusto/python scripts/check-ocr-python.py scripts/native-notices.py scripts/artifact_files.py
	$(PYTHON) scripts/check-ocr-python.py

ocr-python-msrv:
	$(CARGO) +$(MSRV) check --manifest-path backends/rusto/python/Cargo.toml --all-targets --locked
