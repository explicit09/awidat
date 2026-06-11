.PHONY: check check-app check-agent check-desktop-rust fmt fmt-app fmt-agent clippy clippy-app clippy-agent test test-app test-agent python-smoke python-smoke-audio desktop desktop-stop desktop-deps desktop-yt-dlp desktop-ffmpeg desktop-uv desktop-mcp-server desktop-codex desktop-sidecar-check-stubs desktop-codex-check-stub

YT_DLP_VERSION ?= 2026.03.17
FFMPEG_VERSION ?= 7.1.1
UV_VERSION ?= 0.11.14
FFMPEG_MACOS_BASE_URL ?= https://evermeet.cx/ffmpeg
FFMPEG_WINDOWS_URL ?= https://github.com/GyanD/codexffmpeg/releases/download/$(FFMPEG_VERSION)/ffmpeg-$(FFMPEG_VERSION)-full_build.zip
FFMPEG_NPM_BASE_URL ?= https://registry.npmjs.org/@ffmpeg-installer
FFPROBE_NPM_BASE_URL ?= https://registry.npmjs.org/@ffprobe-installer
FFMPEG_STATIC_RELEASE ?= b6.1.1
FFMPEG_STATIC_BASE_URL ?= https://github.com/eugeneware/ffmpeg-static/releases/download/$(FFMPEG_STATIC_RELEASE)
DESKTOP_CARGO_TARGET_DIR ?= ../../target
MONTAGE_SKILLS_ROOT ?= $(CURDIR)/skills

MONTAGE_APP_PACKAGES := \
	-p montage-proto \
	-p montage-core \
	-p montage-tools \
	-p montage-mcp \
	-p montage-sandboxing \
	-p montage-cli \
	-p montage-render \
	-p montage-render-gpu \
	-p montage-effects \
	-p montage-lut \
	-p montage-test-support \
	-p montage-config \
	-p montage-secrets \
	-p montage-social \
	-p montage-social-server \
	-p montage-index \
	-p montage-desktop-protocol

MONTAGE_AGENT_PACKAGES := \
	-p montage-auth \
	-p montage-codex-bridge \
	-p montage-agent-cli

check: fmt clippy test

check-app: fmt-app clippy-app test-app

check-agent: fmt-agent clippy-agent test-agent

check-desktop-rust:
	cargo fmt -p montage-desktop -- --check
	cargo clippy -p montage-desktop --all-targets -- -D warnings
	cargo test -p montage-desktop

fmt:
	cargo fmt --all -- --check

fmt-app:
	cargo fmt $(MONTAGE_APP_PACKAGES) -- --check

fmt-agent:
	cargo fmt $(MONTAGE_AGENT_PACKAGES) -- --check

clippy:
	cargo clippy --workspace --all-targets -- -D warnings

clippy-app:
	cargo clippy $(MONTAGE_APP_PACKAGES) --all-targets -- -D warnings

clippy-agent:
	cargo clippy $(MONTAGE_AGENT_PACKAGES) --all-targets -- -D warnings

test:
	cargo test --workspace

test-app:
	MONTAGE_SKILLS_ROOT="$(MONTAGE_SKILLS_ROOT)" RUST_TEST_THREADS=1 cargo test $(MONTAGE_APP_PACKAGES)

test-agent:
	cargo test $(MONTAGE_AGENT_PACKAGES)

python-smoke:
	python3 python/scripts/smoke_indexers.py --safe

python-smoke-audio:
	python3 python/scripts/smoke_indexers.py --safe --audio-energy

# Montage desktop (Tauri) — install frontend deps + run dev shell.
# Frontend deps live under apps/desktop/node_modules; the Rust
# backend builds via cargo-tauri's `tauri dev` invocation.
desktop-deps:
	cd apps/desktop && pnpm install

# Fetch the standalone yt-dlp binary for a Rust target triple into
# apps/desktop/src-tauri/binaries/. Tauri's externalBin convention
# names the file with the rust target triple as a suffix. Local dev
# defaults to the host triple; release jobs can pass TARGET_TRIPLE.
desktop-yt-dlp:
	@set -e; \
	target_triple="$(TARGET_TRIPLE)"; \
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
	if ! curl --retry 5 --retry-all-errors --retry-delay 2 -fsSL -o "$$dest" "$$url"; then \
	    if [ "$${YT_DLP_ALLOW_PLACEHOLDER:-0}" != "1" ]; then \
	        exit 1; \
	    fi; \
	    echo "yt-dlp download failed; writing CI compile-check placeholder at $$dest" >&2; \
	    printf '%s\n' '#!/usr/bin/env sh' 'echo "yt-dlp sidecar unavailable in CI compile check" >&2' 'exit 127' > "$$dest"; \
	fi; \
	chmod +x "$$dest"; \
	echo "wrote $$dest"

desktop-codex:
	@target_triple="$(TARGET_TRIPLE)"; \
	if [ -z "$$target_triple" ]; then \
	    target_triple="$$(rustc -vV | awk '/^host:/ { print $$2 }')"; \
	fi; \
	codex_profile="$${CODEX_PROFILE:-debug}"; \
	codex_profile_flag=""; \
	if [ "$$codex_profile" = "release" ]; then \
	    codex_profile_flag="--release"; \
	elif [ "$$codex_profile" != "debug" ]; then \
	    echo "unsupported CODEX_PROFILE=$$codex_profile; expected debug or release" >&2; \
	    exit 1; \
	fi; \
	dest="apps/desktop/src-tauri/binaries/codex-$$target_triple"; \
	if [ "$$target_triple" = "x86_64-pc-windows-msvc" ]; then \
	    dest="$$dest.exe"; \
	fi; \
	cargo build -p codex-cli --bin codex --target "$$target_triple" $$codex_profile_flag; \
	mkdir -p "$$(dirname "$$dest")"; \
	cp "target/$$target_triple/$$codex_profile/codex$$(if [ "$$target_triple" = "x86_64-pc-windows-msvc" ]; then echo .exe; fi)" "$$dest"; \
	chmod +x "$$dest"; \
	echo "wrote $$dest"

desktop-codex-check-stub:
	@target_triple="$(TARGET_TRIPLE)"; \
	if [ -z "$$target_triple" ]; then \
	    target_triple="$$(rustc -vV | awk '/^host:/ { print $$2 }')"; \
	fi; \
	dest="apps/desktop/src-tauri/binaries/codex-$$target_triple"; \
	if [ "$$target_triple" = "x86_64-pc-windows-msvc" ]; then \
	    dest="$$dest.exe"; \
	fi; \
	mkdir -p "$$(dirname "$$dest")"; \
	printf '%s\n' '#!/bin/sh' 'echo "codex sidecar check stub; run make desktop-codex for a runnable sidecar" >&2' 'exit 127' > "$$dest"; \
	chmod +x "$$dest"; \
	echo "wrote check stub $$dest"

desktop-sidecar-check-stubs:
	@set -e; \
	target_triple="$(TARGET_TRIPLE)"; \
	if [ -z "$$target_triple" ]; then \
	    target_triple="$$(rustc -vV | awk '/^host:/ { print $$2 }')"; \
	fi; \
	dest_dir="apps/desktop/src-tauri/binaries"; \
	mkdir -p "$$dest_dir"; \
	suffix="-$$target_triple"; \
	if [ "$$target_triple" = "x86_64-pc-windows-msvc" ]; then \
	    suffix="$$suffix.exe"; \
	fi; \
	for sidecar in codex ffmpeg ffprobe montage-mcp-server uv yt-dlp; do \
	    dest="$$dest_dir/$$sidecar$$suffix"; \
	    printf '%s\n' '#!/usr/bin/env sh' "echo \"$$sidecar sidecar check stub; fetch a runnable sidecar before packaging\" >&2" 'exit 127' > "$$dest"; \
	    chmod +x "$$dest"; \
	    echo "wrote check stub $$dest"; \
	done

desktop-ffmpeg:
	@set -e; \
	target_triple="$(TARGET_TRIPLE)"; \
	if [ -z "$$target_triple" ]; then \
	    target_triple="$$(rustc -vV | awk '/^host:/ { print $$2 }')"; \
	fi; \
	dest_dir="apps/desktop/src-tauri/binaries"; \
	mkdir -p "$$dest_dir"; \
		tmp="$$(mktemp -d)"; \
		trap 'rm -rf "$$tmp"' EXIT; \
		fetch_static_sidecars() { \
		    static_platform="$$1"; \
		    ffmpeg_dest="$$dest_dir/ffmpeg-$$target_triple"; \
		    ffprobe_dest="$$dest_dir/ffprobe-$$target_triple"; \
		    if [ -x "$$ffmpeg_dest" ] && [ -x "$$ffprobe_dest" ] && [ "$${FFMPEG_REFRESH:-0}" != "1" ] && ! grep -Eaq "sidecar check stub|sidecar unavailable in CI compile check" "$$ffmpeg_dest" "$$ffprobe_dest"; then \
		        echo "ffmpeg/ffprobe already at $$dest_dir for $$target_triple"; \
		        exit 0; \
		    fi; \
		    echo "fetching $(FFMPEG_STATIC_BASE_URL)/ffmpeg-$$static_platform.gz"; \
		    curl -fsSL -o "$$tmp/ffmpeg.gz" "$(FFMPEG_STATIC_BASE_URL)/ffmpeg-$$static_platform.gz"; \
		    gunzip -c "$$tmp/ffmpeg.gz" > "$$ffmpeg_dest"; \
		    echo "fetching $(FFMPEG_STATIC_BASE_URL)/ffprobe-$$static_platform.gz"; \
		    curl -fsSL -o "$$tmp/ffprobe.gz" "$(FFMPEG_STATIC_BASE_URL)/ffprobe-$$static_platform.gz"; \
		    gunzip -c "$$tmp/ffprobe.gz" > "$$ffprobe_dest"; \
		    chmod +x "$$ffmpeg_dest" "$$ffprobe_dest"; \
		}; \
		case "$$target_triple" in \
		  aarch64-apple-darwin) \
		    fetch_static_sidecars darwin-arm64; \
		    ;; \
	  x86_64-apple-darwin) \
	    ffmpeg_dest="$$dest_dir/ffmpeg-$$target_triple"; \
	    ffprobe_dest="$$dest_dir/ffprobe-$$target_triple"; \
	    if [ -x "$$ffmpeg_dest" ] && [ -x "$$ffprobe_dest" ] && [ "$${FFMPEG_REFRESH:-0}" != "1" ]; then \
	        if "$$ffmpeg_dest" -version 2>/dev/null | head -n 1 | grep -q "ffmpeg version $(FFMPEG_VERSION)" && "$$ffprobe_dest" -version 2>/dev/null | head -n 1 | grep -q "ffprobe version $(FFMPEG_VERSION)"; then \
	            echo "ffmpeg/ffprobe $(FFMPEG_VERSION) already at $$dest_dir for $$target_triple"; \
	            exit 0; \
	        fi; \
	    fi; \
	    curl -fsSL -o "$$tmp/ffmpeg.zip" "$(FFMPEG_MACOS_BASE_URL)/ffmpeg-$(FFMPEG_VERSION).zip"; \
	    curl -fsSL -o "$$tmp/ffprobe.zip" "$(FFMPEG_MACOS_BASE_URL)/ffprobe-$(FFMPEG_VERSION).zip"; \
	    unzip -p "$$tmp/ffmpeg.zip" ffmpeg > "$$ffmpeg_dest"; \
	    unzip -p "$$tmp/ffprobe.zip" ffprobe > "$$ffprobe_dest"; \
	    chmod +x "$$ffmpeg_dest" "$$ffprobe_dest"; \
	    ;; \
		  x86_64-unknown-linux-gnu) \
		    fetch_static_sidecars linux-x64; \
		    ;; \
		  aarch64-unknown-linux-gnu) \
		    fetch_static_sidecars linux-arm64; \
		    ;; \
	  x86_64-pc-windows-msvc) \
	    ffmpeg_dest="$$dest_dir/ffmpeg-$$target_triple.exe"; \
	    ffprobe_dest="$$dest_dir/ffprobe-$$target_triple.exe"; \
	    if [ -s "$$ffmpeg_dest" ] && [ -s "$$ffprobe_dest" ] && [ "$${FFMPEG_REFRESH:-0}" != "1" ] && ! grep -Eaq "sidecar check stub|sidecar unavailable in CI compile check" "$$ffmpeg_dest" "$$ffprobe_dest"; then \
	        echo "ffmpeg/ffprobe $(FFMPEG_VERSION) already at $$dest_dir for $$target_triple"; \
	        exit 0; \
	    fi; \
	    curl -fsSL -o "$$tmp/ffmpeg-windows.zip" "$(FFMPEG_WINDOWS_URL)"; \
	    unzip -p "$$tmp/ffmpeg-windows.zip" "ffmpeg-$(FFMPEG_VERSION)-full_build/bin/ffmpeg.exe" > "$$ffmpeg_dest"; \
	    unzip -p "$$tmp/ffmpeg-windows.zip" "ffmpeg-$(FFMPEG_VERSION)-full_build/bin/ffprobe.exe" > "$$ffprobe_dest"; \
	    chmod +x "$$ffmpeg_dest" "$$ffprobe_dest"; \
	    ;; \
	  *) echo "unknown ffmpeg target triple: $$target_triple" >&2; exit 1 ;; \
	esac; \
	echo "wrote $$ffmpeg_dest"; \
	echo "wrote $$ffprobe_dest"

desktop-uv:
	@set -e; \
	target_triple="$(TARGET_TRIPLE)"; \
	if [ -z "$$target_triple" ]; then \
	    target_triple="$$(rustc -vV | awk '/^host:/ { print $$2 }')"; \
	fi; \
	dest_dir="apps/desktop/src-tauri/binaries"; \
	mkdir -p "$$dest_dir"; \
	tmp="$$(mktemp -d)"; \
	trap 'rm -rf "$$tmp"' EXIT; \
	case "$$target_triple" in \
	  aarch64-apple-darwin|x86_64-apple-darwin|x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu) \
	    uv_dest="$$dest_dir/uv-$$target_triple"; \
	    if [ -x "$$uv_dest" ] && [ "$${UV_REFRESH:-0}" != "1" ]; then \
	        if "$$uv_dest" --version 2>/dev/null | grep -q "uv $(UV_VERSION)"; then \
	            echo "uv $(UV_VERSION) already at $$uv_dest"; \
	            exit 0; \
	        fi; \
	    fi; \
	    archive="uv-$$target_triple.tar.gz"; \
	    curl -fsSL -o "$$tmp/uv.tar.gz" "https://github.com/astral-sh/uv/releases/download/$(UV_VERSION)/$$archive"; \
	    tar -xzf "$$tmp/uv.tar.gz" -C "$$tmp"; \
	    cp "$$tmp/uv-$$target_triple/uv" "$$uv_dest"; \
	    chmod +x "$$uv_dest"; \
	    ;; \
	  x86_64-pc-windows-msvc) \
	    uv_dest="$$dest_dir/uv-$$target_triple.exe"; \
	    if [ -s "$$uv_dest" ] && [ "$${UV_REFRESH:-0}" != "1" ] && ! grep -Eaq "sidecar check stub|sidecar unavailable in CI compile check" "$$uv_dest"; then \
	        echo "uv $(UV_VERSION) already at $$uv_dest"; \
	        exit 0; \
	    fi; \
	    archive="uv-$$target_triple.zip"; \
	    curl -fsSL -o "$$tmp/uv.zip" "https://github.com/astral-sh/uv/releases/download/$(UV_VERSION)/$$archive"; \
	    unzip -p "$$tmp/uv.zip" "uv-$$target_triple/uv.exe" > "$$uv_dest"; \
	    chmod +x "$$uv_dest"; \
	    ;; \
	  *) echo "unknown uv target triple: $$target_triple" >&2; exit 1 ;; \
	esac; \
	echo "wrote $$uv_dest"

desktop-mcp-server:
	@set -e; \
	target_triple="$(TARGET_TRIPLE)"; \
	if [ -z "$$target_triple" ]; then \
	    target_triple="$$(rustc -vV | awk '/^host:/ { print $$2 }')"; \
	fi; \
	cargo_target_dir="$${CARGO_TARGET_DIR:-$(DESKTOP_CARGO_TARGET_DIR)}"; \
	CARGO_TARGET_DIR="$$cargo_target_dir" cargo build -p montage-cli --bin montage-mcp-server --release --target "$$target_triple"; \
	dest="apps/desktop/src-tauri/binaries/montage-mcp-server-$$target_triple"; \
	source="$$cargo_target_dir/$$target_triple/release/montage-mcp-server"; \
	if echo "$$target_triple" | grep -q 'windows'; then \
	    dest="$$dest.exe"; \
	    source="$$source.exe"; \
	fi; \
	mkdir -p "$$(dirname "$$dest")"; \
	cp "$$source" "$$dest"; \
	chmod +x "$$dest"; \
	echo "wrote $$dest"

desktop: desktop-deps desktop-yt-dlp desktop-ffmpeg desktop-uv desktop-mcp-server desktop-codex
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
