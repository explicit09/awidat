# Awidat Homebrew Formula (template).
#
# This is the template that the release workflow auto-bumps. The
# `version`, `url`, and `sha256` fields per platform are filled in
# by .github/workflows/release.yml when a `v*` tag is pushed; the
# bumped formula is then committed to the explicit09/homebrew-awidat
# tap repo.
#
# End-user install (after the tap repo + first release exist):
#
#   brew install explicit09/awidat/awidat
#
# What the formula does:
#  - Downloads the platform-matching tarball + SHA256-verifies it.
#  - Installs the awidat binary into HOMEBREW_PREFIX/bin/.
#  - Installs the bundled python tree + uv into libexec/.
#  - Wraps awidat with a shim that sets AWIDAT_PYTHON_ROOT to the
#    libexec/python location, so path resolution works regardless
#    of brew prefix (/opt/homebrew on Apple Silicon, /usr/local on
#    Intel, ~/.linuxbrew/Homebrew on Linuxbrew).
#  - Runs `uv sync --all-packages` post-install to materialize
#    indexer venvs.
#
# Dependencies declared:
#  - ffmpeg: required by every render path + most indexers
#  - yt-dlp: required by `awidat new --import <youtube-url>`
#  - uv: redundant with the bundled uv binary, but listed so users
#    have a system-PATH uv too (some workflows expect it)

class Awidat < Formula
  desc "Terminal-first agent for editing long-form spoken video"
  homepage "https://github.com/explicit09/awidat"

  # AUTO-BUMPED FIELDS BELOW.
  # The release workflow rewrites this version + per-platform URL
  # and sha256 on every tagged release. Don't hand-edit; PRs
  # against this template should preserve the AUTOBUMP markers.

  # AUTOBUMP_VERSION_BEGIN
  version "0.0.0"
  # AUTOBUMP_VERSION_END

  on_macos do
    on_arm do
      # AUTOBUMP_MACOS_ARM_BEGIN
      url "https://github.com/explicit09/awidat/releases/download/v0.0.0/awidat-aarch64-apple-darwin.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
      # AUTOBUMP_MACOS_ARM_END
    end
    on_intel do
      # AUTOBUMP_MACOS_INTEL_BEGIN
      url "https://github.com/explicit09/awidat/releases/download/v0.0.0/awidat-x86_64-apple-darwin.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
      # AUTOBUMP_MACOS_INTEL_END
    end
  end

  on_linux do
    on_arm do
      # AUTOBUMP_LINUX_ARM_BEGIN
      url "https://github.com/explicit09/awidat/releases/download/v0.0.0/awidat-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
      # AUTOBUMP_LINUX_ARM_END
    end
    on_intel do
      # AUTOBUMP_LINUX_INTEL_BEGIN
      url "https://github.com/explicit09/awidat/releases/download/v0.0.0/awidat-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
      # AUTOBUMP_LINUX_INTEL_END
    end
  end

  depends_on "ffmpeg"
  depends_on "uv"
  depends_on "yt-dlp"

  def install
    # Tarball layout (per dist/package.sh):
    #   bin/awidat
    #   bin/uv
    #   python/...
    #   share/awidat/install.sh
    #   VERSION

    # Stage the python tree + bundled uv into libexec so they're
    # not on $PATH and don't conflict with brew-managed uv.
    libexec.install "python", "VERSION"

    # The actual binary lives under bin/, but we wrap it so
    # AWIDAT_PYTHON_ROOT points at libexec/python regardless of
    # which brew prefix the user has. Without this wrapper, awidat's
    # built-in python_root() resolution falls back to walking up
    # from the binary, which works for ~/.local/share/awidat installs
    # but not for brew (binary is at HOMEBREW_PREFIX/bin/awidat,
    # python is at HOMEBREW_PREFIX/Cellar/awidat/<v>/libexec/python).
    libexec.install "bin/awidat" => "awidat-bin"

    (bin/"awidat").write <<~SH
      #!/usr/bin/env bash
      export AWIDAT_PYTHON_ROOT="#{libexec}/python"
      exec "#{libexec}/awidat-bin" "$@"
    SH
    (bin/"awidat").chmod 0755
  end

  def post_install
    # Materialize per-indexer venvs. uv's wheel cache lives under
    # ~/.cache/uv/ — so subsequent installs (after the first) are
    # near-instant. First install pulls ~3 GB of torch + dlib +
    # opencv wheels.
    ohai "Resolving python indexers (one-time ~3 GB on first install)..."
    system Formula["uv"].opt_bin/"uv", "sync", "--all-packages",
           "--directory", "#{libexec}/python"
  end

  def caveats
    <<~EOS
      First-time setup:
        awidat secrets-set            # stash your Anthropic API key
                                      # in the OS keychain (one-time)

      Try it:
        awidat new my-cut --import https://youtu.be/<id>

      The python indexer tree was materialized into:
        #{libexec}/python

      Override AWIDAT_PYTHON_ROOT in your shell to point at a
      different python workspace (useful for development).
    EOS
  end

  test do
    # Simple smoke. The full TUI/agent loop requires an API key
    # so we only verify the binary runs and reports its version.
    assert_match "awidat", shell_output("#{bin}/awidat version")
  end
end
