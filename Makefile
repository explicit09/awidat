.PHONY: check fmt clippy test python-smoke python-smoke-audio desktop desktop-stop desktop-deps desktop-yt-dlp

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

# Stop stale dev processes that can keep Vite's fixed Tauri port busy.
desktop-stop:
	@pids="$$(lsof -tiTCP:1420 -sTCP:LISTEN 2>/dev/null || true)"; \
	if [ -z "$$pids" ]; then \
	    echo "no process is listening on port 1420"; \
	else \
	    echo "stopping dev server on port 1420: $$pids"; \
	    kill $$pids; \
	fi
