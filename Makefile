.PHONY: check fmt clippy test package install-local clean-dist desktop desktop-deps desktop-yt-dlp

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

# Awidat desktop (Tauri) — install frontend deps + run dev shell.
# Frontend deps live under apps/desktop/node_modules; the Rust
# backend builds via cargo-tauri's `tauri dev` invocation.
desktop-deps:
	cd apps/desktop && pnpm install

# Fetch the standalone yt-dlp binary for the host triple into
# apps/desktop/src-tauri/binaries/. Tauri's externalBin convention
# names the file with the rust target triple as a suffix; we only
# fetch the host's triple in dev. CI populates the others on release.
desktop-yt-dlp:
	@host_triple="$$(rustc -vV | awk '/^host:/ { print $$2 }')"; \
	dest="apps/desktop/src-tauri/binaries/yt-dlp-$$host_triple"; \
	if [ -x "$$dest" ]; then \
	    echo "yt-dlp already at $$dest"; \
	    exit 0; \
	fi; \
	mkdir -p "$$(dirname "$$dest")"; \
	case "$$host_triple" in \
	  aarch64-apple-darwin)         url='https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_macos' ;; \
	  x86_64-apple-darwin)          url='https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_macos_legacy' ;; \
	  x86_64-unknown-linux-gnu)     url='https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux' ;; \
	  aarch64-unknown-linux-gnu)    url='https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux_aarch64' ;; \
	  x86_64-pc-windows-msvc)       dest="$$dest.exe"; url='https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe' ;; \
	  *) echo "unknown host triple: $$host_triple" >&2; exit 1 ;; \
	esac; \
	echo "fetching $$url"; \
	curl -fsSL -o "$$dest" "$$url" && chmod +x "$$dest"; \
	echo "wrote $$dest"

desktop: desktop-deps desktop-yt-dlp
	cd apps/desktop && pnpm tauri dev
