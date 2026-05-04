.PHONY: check fmt clippy test package install-local clean-dist

check: fmt clippy test

fmt:
	cargo fmt --all -- --check

clippy:
	cargo clippy --workspace --all-targets -- -D warnings

test:
	cargo test --workspace

# Build a release tarball for the host platform under dist/build/.
# See dist/README.md for the distribution model + tradeoffs.
package:
	./dist/package.sh

# Build + install onto this machine for testing the install flow
# end-to-end. Useful when validating that the dist/install.sh
# script works after iterating on package.sh output layout.
install-local: package
	@triple="$$(uname -s | tr '[:upper:]' '[:lower:]')-$$(uname -m)"; \
	case "$$triple" in \
	  darwin-arm64)  triple=aarch64-apple-darwin ;; \
	  darwin-x86_64) triple=x86_64-apple-darwin ;; \
	  linux-x86_64)  triple=x86_64-unknown-linux-gnu ;; \
	  linux-aarch64) triple=aarch64-unknown-linux-gnu ;; \
	esac; \
	AWIDAT_RELEASE_BASE="file://$$(pwd)/dist/build" \
	  bash "dist/build/awidat-$$triple/share/awidat/install.sh"

clean-dist:
	rm -rf dist/build
