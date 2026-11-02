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
ADAPTER_SURFACE := api/roomkit-adapter-surface.json

.PHONY: all fmt fmt-check clippy test doc deny check-core-deps examples msrv msrv-toolchain hooks roomkit-check roomkit-msrv python-check python-msrv api-snapshot api-test api-adapter-surface api-adapter-check

all: fmt-check clippy test doc deny check-core-deps examples

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
	uv tool run --from 'ruff>=0.14,<1' ruff check python tests/python examples scripts/check-python.py scripts/native-notices.py scripts/artifact_files.py scripts/api-snapshot.py
	uv tool run --from 'ruff>=0.14,<1' ruff format --check python tests/python examples scripts/check-python.py scripts/native-notices.py scripts/artifact_files.py scripts/api-snapshot.py
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
