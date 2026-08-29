#!/bin/sh
# jellysink installer
# Usage: curl -fsSL https://raw.githubusercontent.com/Stax124/jellysink/main/install.sh | sh
#        curl -fsSL ... | sh -s -- --no-systemd
#
# Downloads the latest jellysink release from GitHub and installs the
# binary to ~/.local/bin (the path the user systemd unit expects).
# Also installs systemd/jellysink.service unless --no-systemd is passed.

set -e

REPO="Stax124/jellysink"
BINARY="jellysink"
INSTALL_DIR="${HOME}/.local/bin"
UNIT_DIR="${HOME}/.config/systemd/user"
INSTALL_UNIT="1"

# --- helpers ---

info() { printf '  \033[1;34m>\033[0m %s\n' "$*"; }
warn() { printf '  \033[1;33m>\033[0m %s\n' "$*"; }
err()  { printf '  \033[1;31m!\033[0m %s\n' "$*" >&2; exit 1; }

need() {
    command -v "$1" >/dev/null 2>&1 || err "Required tool '$1' not found. Please install it and try again."
}

# --- parse arguments ---

parse_args() {
    while [ $# -gt 0 ]; do
        case "$1" in
            --help|-h)
                echo "Usage: install.sh [OPTIONS]"
                echo ""
                echo "Downloads the latest jellysink release and installs it to ~/.local/bin."
                echo "Also installs a user systemd unit unless --no-systemd is passed."
                echo ""
                echo "Options:"
                echo "  --help, -h       Show this help message"
                echo "  --no-systemd     Skip installing the user systemd unit"
                exit 0
                ;;
            --no-systemd)
                INSTALL_UNIT=""
                ;;
            *)
                warn "Unknown option: $1"
                ;;
        esac
        shift
    done
}

# --- detect platform ---

detect_platform() {
    OS="$(uname -s)"
    ARCH="$(uname -m)"

    case "$OS" in
        Linux) ;;
        *) err "Unsupported OS: $OS (jellysink is Linux-only)" ;;
    esac

    case "$ARCH" in
        x86_64|amd64)   ARCH="x86_64" ;;
        aarch64|arm64)  ARCH="aarch64" ;;
        *)              err "Unsupported architecture: $ARCH" ;;
    esac

    PLATFORM="${ARCH}-unknown-linux-musl"
}

# --- fetch latest release ---

fetch_latest_tag() {
    need curl

    # Use the releases redirect instead of the API to avoid GitHub's
    # 60-request/hour rate limit on unauthenticated API calls (403).
    TAG="$(curl -fsSI "https://github.com/${REPO}/releases/latest" 2>/dev/null \
        | grep -i '^location:' \
        | head -n 1 \
        | sed 's|.*/tag/||' \
        | tr -d '\r\n')"

    [ -n "$TAG" ] || err "Could not determine latest release. Check https://github.com/${REPO}/releases"
}

# --- checksum verification ---

verify_checksum() {
    CHECKSUM_FILE="${workdir}/${ASSET}.sha256"

    # Attempt to download the checksum file (-f exits non-zero on HTTP 4xx/5xx)
    if ! curl -fsSL --max-time 10 "${URL}.sha256" -o "$CHECKSUM_FILE" 2>/dev/null; then
        warn "No checksum file found for this release — skipping integrity check"
        return
    fi

    info "Verifying checksum..."
    if command -v sha256sum >/dev/null 2>&1; then
        (cd "$workdir" && sha256sum -c "${ASSET}.sha256" --quiet) \
            || err "Checksum verification failed. The download may be corrupted or tampered with."
    elif command -v shasum >/dev/null 2>&1; then
        (cd "$workdir" && shasum -a 256 -q -c "${ASSET}.sha256") \
            || err "Checksum verification failed. The download may be corrupted or tampered with."
    else
        warn "Neither sha256sum nor shasum available — skipping integrity check"
    fi
}

# --- download and install ---

install_binary() {
    ASSET="${BINARY}-${PLATFORM}"
    URL="https://github.com/${REPO}/releases/download/${TAG}/${ASSET}"

    workdir="$(mktemp -d)"
    trap 'rm -rf "$workdir"' EXIT

    info "Downloading ${BINARY} ${TAG} for ${PLATFORM}..."
    curl -fsSL "$URL" -o "${workdir}/${ASSET}" \
        || err "Download failed. Asset '${ASSET}' may not exist for your platform.
  Check: https://github.com/${REPO}/releases/tag/${TAG}"

    verify_checksum

    chmod +x "${workdir}/${ASSET}"

    mkdir -p "$INSTALL_DIR"
    info "Installing to ${INSTALL_DIR}..."
    mv "${workdir}/${ASSET}" "${INSTALL_DIR}/${BINARY}"
    info "Installed ${BINARY} to ${INSTALL_DIR}/${BINARY}"

    case ":$PATH:" in
        *":${INSTALL_DIR}:"*) ;;
        *)
            warn "Add ${INSTALL_DIR} to your PATH to use '${BINARY}' directly:"
            echo ""
            echo "    export PATH=\"\$HOME/.local/bin:\$PATH\""
            echo ""
            ;;
    esac
}

install_unit() {
    [ -n "$INSTALL_UNIT" ] || return 0

    UNIT_URL="https://raw.githubusercontent.com/${REPO}/${TAG}/systemd/jellysink.service"
    mkdir -p "$UNIT_DIR"

    info "Installing user systemd unit..."
    if ! curl -fsSL "$UNIT_URL" -o "${UNIT_DIR}/jellysink.service"; then
        warn "Could not download systemd unit from ${UNIT_URL}"
        return 0
    fi
    info "Installed ${UNIT_DIR}/jellysink.service"

    if command -v systemctl >/dev/null 2>&1; then
        systemctl --user daemon-reload 2>/dev/null \
            || warn "systemctl --user daemon-reload failed; run it after logging into a graphical session"
    fi
}

# --- main ---

main() {
    parse_args "$@"
    info "jellysink installer"
    detect_platform
    fetch_latest_tag
    install_binary
    install_unit

    if ! command -v mpv >/dev/null 2>&1; then
        warn "mpv is not on PATH. jellysink needs mpv to play — install it from your distro."
    fi

    echo ""
    info "Done. Next:"
    echo ""
    echo "    jellysink login"
    if [ -n "$INSTALL_UNIT" ]; then
        echo "    systemctl --user enable --now jellysink"
    else
        echo "    jellysink run"
    fi
    echo ""
}

main "$@"
