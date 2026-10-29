CARGO ?= cargo
MSRV := $(shell sed -n 's/^rust-version = "\(.*\)"$$/\1/p' Cargo.toml).0
EXAMPLES := $(sort $(basename $(notdir $(wildcard examples/*.rs))))
DENY_FLAGS ?=
ROOMKIT_MANIFEST := adapters/roomkit/Cargo.toml
PYTHON_MANIFEST := bindings/python/Cargo.toml
PYTHON ?= python3

.PHONY: all fmt fmt-check clippy test doc deny check-core-deps examples msrv msrv-toolchain hooks roomkit-check roomkit-msrv python-check python-msrv

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
	$(CARGO) clippy --manifest-path $(ROOMKIT_MANIFEST) --all-targets --locked -- -D warnings
	$(CARGO) test --manifest-path $(ROOMKIT_MANIFEST) --locked
	RUSTDOCFLAGS="-D warnings" $(CARGO) doc --manifest-path $(ROOMKIT_MANIFEST) --no-deps --locked

roomkit-msrv:
	$(CARGO) +$(MSRV) check --manifest-path $(ROOMKIT_MANIFEST) --all-targets --locked

# Optional CPython bridge: the standalone Rust gates remain interpreter-free.
python-check:
	$(CARGO) fmt --manifest-path $(PYTHON_MANIFEST) --all -- --check
	$(CARGO) clippy --manifest-path $(PYTHON_MANIFEST) --all-targets --locked -- -D warnings
	RUSTDOCFLAGS="-D warnings" $(CARGO) doc --manifest-path $(PYTHON_MANIFEST) --no-deps --locked
	$(CARGO) deny --manifest-path $(PYTHON_MANIFEST) --config deny.toml check
	uv tool run --from 'ruff>=0.14,<1' ruff check python tests/python examples scripts/check-python.py
	uv tool run --from 'ruff>=0.14,<1' ruff format --check python tests/python examples scripts/check-python.py
	$(PYTHON) scripts/check-python.py

python-msrv:
	$(CARGO) +$(MSRV) check --manifest-path $(PYTHON_MANIFEST) --all-targets --locked
