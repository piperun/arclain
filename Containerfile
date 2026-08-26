# Arclain dev/CI container — mirrors Woodpecker's exact Rust step so
# you can reproduce any CI step locally with warm caches.
#
# Build:
#   podman build -t arclain-dev -f Containerfile .
#
# Run (one-shot CI-step equivalents via compose, see compose.yaml):
#   podman compose run --rm check
#   podman compose run --rm test
#   podman compose run --rm plugins
#   podman compose run --rm build-linux
#
# Interactive shell with everything mounted:
#   podman compose run --rm dev
#
# All three cache dirs (target/, cargo registry, cargo git) live in named
# volumes (compose.yaml), so the first build is slow and every subsequent
# command starts warm. Removing the volumes wipes the cache:
#   podman volume rm arclain-target arclain-cargo-registry arclain-cargo-git

FROM docker.io/library/rust:1.98.0-bookworm

# ── System deps — copy-paste from .woodpecker.yml so a Containerfile drift
#    can't silently diverge from CI. Add python3 for scripts/release.py.
RUN apt-get update -qq && \
    apt-get install -yqq --no-install-recommends \
        libgtk-3-dev \
        libxcb-render0-dev \
        libxcb-shape0-dev \
        libxcb-xfixes0-dev \
        libxkbcommon-dev \
        libssl-dev \
        pkg-config \
        python3 \
        curl \
        ca-certificates \
        git && \
    rm -rf /var/lib/apt/lists/*

# ── Rust extras — wasm32-wasip2 for plugin builds, cargo-nextest from
#    prebuilt binary (same trick as .woodpecker.yml cargo-test step —
#    avoids ~2 min of cold compile every container build).
RUN rustup target add wasm32-wasip2 && \
    curl -LsSf https://get.nexte.st/latest/linux | \
        tar xzf - -C /usr/local/cargo/bin

# ── Non-root runner user. Matches the CI pattern so chmod-permission
#    tests (which use root-bypassed 0o500/0o555 bits) behave identically
#    here as they do in woodpecker. UID 1000 matches the typical first
#    Fedora user — keeps bind-mount file ownership clean without
#    needing :Z relabel hacks.
RUN useradd -m -u 1000 -s /bin/bash runner

# ── Pre-create the workspace dir as the runner so bind-mounts inherit
#    the right ownership and cargo doesn't need to chown registry on
#    first run.
RUN mkdir -p /workspace/codeberg && \
    chown -R runner:runner /workspace /usr/local/cargo

USER runner
WORKDIR /workspace/codeberg/arclain

# Belt-and-suspenders against the user's host clang+lld linker pinning
# (their global ~/.cargo/config.toml has linker = "clang"). Inside the
# container, default gcc is fine and clang isn't installed.
ENV CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
ENV CARGO_ENCODED_RUSTFLAGS=""

# Default to an interactive shell. compose.yaml overrides this per
# service for the CI-step shortcuts.
CMD ["/bin/bash"]
