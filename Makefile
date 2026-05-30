.PHONY: check fmt clippy test python-smoke python-smoke-audio desktop desktop-stop desktop-deps desktop-yt-dlp

YT_DLP_VERSION ?= 2026.03.17

check: fmt clippy test

fmt:
	cargo fmt --all -- --check

clippy:
	cargo clippy --workspace --all-targets -- -D warnings

test:
	cargo test --workspace

python-smoke:
	python3 python/scripts/smoke_indexers.py --safe

python-smoke-audio:
	python3 python/scripts/smoke_indexers.py --safe --audio-energy

# Awidat desktop (Tauri) — install frontend deps + run dev shell.
# Frontend deps live under apps/desktop/node_modules; the Rust
# backend builds via cargo-tauri's `tauri dev` invocation.
desktop-deps:
	cd apps/desktop && pnpm install

# Fetch the standalone yt-dlp binary for a Rust target triple into
# apps/desktop/src-tauri/binaries/. Tauri's externalBin convention
# names the file with the rust target triple as a suffix. Local dev
# defaults to the host triple; release jobs can pass TARGET_TRIPLE.
desktop-yt-dlp:
	@target_triple="$(TARGET_TRIPLE)"; \
	if [ -z "$$target_triple" ]; then \
	    target_triple="$$(rustc -vV | awk '/^host:/ { print $$2 }')"; \
	fi; \
	dest="apps/desktop/src-tauri/binaries/yt-dlp-$$target_triple"; \
	mkdir -p "$$(dirname "$$dest")"; \
	case "$$target_triple" in \
	  aarch64-apple-darwin)         asset='yt-dlp_macos' ;; \
	  x86_64-apple-darwin)          asset='yt-dlp_macos' ;; \
	  x86_64-unknown-linux-gnu)     asset='yt-dlp_linux' ;; \
	  aarch64-unknown-linux-gnu)    asset='yt-dlp_linux_aarch64' ;; \
	  x86_64-pc-windows-msvc)       dest="$$dest.exe"; asset='yt-dlp.exe' ;; \
	  *) echo "unknown target triple: $$target_triple" >&2; exit 1 ;; \
	esac; \
	url="https://github.com/yt-dlp/yt-dlp/releases/download/$(YT_DLP_VERSION)/$$asset"; \
	if [ -x "$$dest" ] && [ "$${YT_DLP_REFRESH:-0}" != "1" ]; then \
	    existing_version="$$("$$dest" --version 2>/dev/null || true)"; \
	    if [ "$$existing_version" = "$(YT_DLP_VERSION)" ]; then \
	        echo "yt-dlp $(YT_DLP_VERSION) already at $$dest"; \
	        exit 0; \
	    fi; \
	    echo "refreshing yt-dlp at $$dest from $${existing_version:-unknown} to $(YT_DLP_VERSION)"; \
	fi; \
	echo "fetching $$url"; \
	curl -fsSL -o "$$dest" "$$url" && chmod +x "$$dest"; \
	echo "wrote $$dest"

desktop: desktop-deps desktop-yt-dlp
	cd apps/desktop && pnpm tauri dev

# Stop stale dev processes that can keep Vite's fixed Tauri port busy.
desktop-stop:
	@pids="$$(lsof -tiTCP:1420 -sTCP:LISTEN 2>/dev/null || true)"; \
	if [ -z "$$pids" ]; then \
	    echo "no process is listening on port 1420"; \
	else \
	    echo "stopping dev server on port 1420: $$pids"; \
	    kill $$pids; \
	fi
