# The typst compiler. `make build` produces the module; `make release`
# publishes it for the application to pin.
#
# The module is built for wasm32-unknown-unknown and lands in dist/ under the
# name a host loads it by. `rustup target add wasm32-unknown-unknown` once.

MODULE  := typst.wasm
TARGET  := target/wasm32-unknown-unknown/release/wasm_typst.wasm
VERSION := $(shell grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
SOURCES := $(shell find src Cargo.toml -type f 2>/dev/null) $(shell find ../wasm-helpers/src ../wasm-helpers/document.css -type f 2>/dev/null)
# Shared Node publisher and encrypted S3-compatible credentials live in the
# sibling application checkout. Both paths can be overridden by callers.
PUBLISHER ?= ../librepaper/tools/publish-mirror.mjs
KEYS      ?= ../librepaper/deploy/keys.yaml

.DEFAULT_GOAL := help
.PHONY: help build compress checksums release mirror mirror-push secrets test fmt clean

help:  ## Display this help screen
	@printf "\033[1mAvailable commands:\033[0m\n\n"
	@grep -hE '^[a-z.A-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) \
		| awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-12s\033[0m %s\n", $$1, $$2}' | sort

build: dist/$(MODULE)  ## Build the module

dist/$(MODULE): $(SOURCES)
	@cargo build --release --target wasm32-unknown-unknown
	@mkdir -p dist
	@cp $(TARGET) $@
	@ls -lh $@ | awk '{print "$(MODULE)", $$5}'

# Compressed once here rather than per request by whatever serves it. Needs
# node, which is the only thing in this repository that does; a build without
# it still produces the module.
compress: dist/$(MODULE).br  ## Pre-compress the module for serving

dist/$(MODULE).br: dist/$(MODULE) tools/compress.mjs
	@node tools/compress.mjs $<

# What the application pins. A module is addressed by the digest of its own
# bytes on both sides of the split, so this file is the whole contract: if the
# digest matches, the binary is serving the module this repository released.
checksums: dist/SHA256SUMS  ## Write the digests the application pins

dist/SHA256SUMS: dist/$(MODULE).br
	@cd dist && sha256sum $(MODULE) $(MODULE).br $(MODULE).gz > SHA256SUMS
	@cat $@

release: checksums  ## Publish the version in Cargo.toml as a GitHub release
	@command -v gh >/dev/null || { echo "gh is not installed"; exit 1; }
	@gh auth status >/dev/null 2>&1 || { echo "gh is not signed in: gh auth login"; exit 1; }
	@git diff --quiet || { echo "working tree is dirty; commit before releasing"; exit 1; }
	@gh release view v$(VERSION) >/dev/null 2>&1 \
		&& { echo "v$(VERSION) is already released; bump version in Cargo.toml"; exit 1; } || true
	@gh release create v$(VERSION) \
		dist/$(MODULE) dist/$(MODULE).br dist/$(MODULE).gz dist/SHA256SUMS \
		--title "$(MODULE) $(VERSION)" \
		--notes "$$(printf 'Built from %s\n\n```\n%s\n```\n' "$$(git rev-parse --short HEAD)" "$$(cat dist/SHA256SUMS)")"

mirror: dist/$(MODULE) tools/mirror.mjs LICENSE NOTICE NOTICE.upstream  ## Build the content-addressed WASM mirror
	@node tools/mirror.mjs $< mirror

mirror-push: mirror  ## Publish the WASM mirror to the configured S3-compatible bucket
	@node "$(PUBLISHER)" --dir mirror --prefix typst

# A target cannot export into the shell that ran make, so this opens a
# subshell with the keys decrypted in its environment; exit it to drop them.
# For one command instead of a shell: sops exec-env $(KEYS) '<command>'
secrets:  ## Open a shell with the sops-encrypted keys in its environment
	@test -f $(KEYS) || { echo "no $(KEYS) -- clone LibrePaper/librepaper beside this repo, or set KEYS="; exit 1; }
	@test -t 0 || { echo "make secrets opens an interactive subshell and needs a terminal" >&2; exit 2; }
	@echo "$(KEYS) is loaded in this shell; exit to drop it"
	@sops exec-env $(KEYS) "$${SHELL:-/bin/sh}"

test:  ## Run the renderer's tests natively
	@cargo test

fmt:
	@cargo fmt

clean:
	@rm -rf target dist
