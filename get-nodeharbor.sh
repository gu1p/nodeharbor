#!/usr/bin/env bash
set -euo pipefail

nodeharbor_error() { printf 'NodeHarbor: %s\n' "$*" >&2; return 1; }

nodeharbor_target() {
  local system cpu
  system=$(uname -s)
  cpu=$(uname -m)
  case "$system" in
    Darwin)
      # uname reports x86_64 in a Rosetta shell. Prefer the physical CPU.
      if [[ $(sysctl -n hw.optional.arm64 2>/dev/null || true) == 1 ]]; then cpu=arm64; fi
      case "$cpu" in
        arm64|aarch64) printf '%s\n' aarch64-apple-darwin ;;
        x86_64) printf '%s\n' x86_64-apple-darwin ;;
        *) nodeharbor_error "Unsupported macOS CPU: $cpu" ;;
      esac ;;
    Linux)
      case "$cpu" in
        arm64|aarch64) printf '%s\n' aarch64-unknown-linux-gnu ;;
        x86_64) printf '%s\n' x86_64-unknown-linux-gnu ;;
        *) nodeharbor_error "Unsupported Linux CPU: $cpu" ;;
      esac ;;
    *) nodeharbor_error "Unsupported operating system: $system. Windows uses get-nodeharbor.ps1." ;;
  esac
}

nodeharbor_hash() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}';
  else shasum -a 256 "$1" | awk '{print $1}'; fi
}

nodeharbor_download() { curl --proto '=https' --tlsv1.2 --fail --silent --show-error --location --retry 3 "$1" -o "$2"; }

NODEHARBOR_TEMP_WORK=''
NODEHARBOR_BACKUP=''
NODEHARBOR_DESTINATION=''
nodeharbor_cleanup() {
  if [[ -n "$NODEHARBOR_BACKUP" && -e "$NODEHARBOR_BACKUP" && ! -e "$NODEHARBOR_DESTINATION" ]]; then
    mv "$NODEHARBOR_BACKUP" "$NODEHARBOR_DESTINATION"
  fi
  if [[ -n "$NODEHARBOR_TEMP_WORK" ]]; then rm -rf -- "$NODEHARBOR_TEMP_WORK"; fi
}

nodeharbor_main() {
  local target version name base expected actual install_dir stage binary agent
  target=$(nodeharbor_target)
  version=${NODEHARBOR_VERSION:-}
  if [[ -z "$version" ]]; then
    version=$(curl --proto '=https' --tlsv1.2 --fail --silent --show-error --location -o /dev/null -w '%{url_effective}' https://github.com/gu1p/nodeharbor/releases/latest)
    version=${version##*/}
  fi
  version=${version#v}
  [[ "$version" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] || nodeharbor_error 'No published release was found, or NODEHARBOR_VERSION is invalid'
  name="nodeharbor-v${version}-${target}"
  case "$target" in
    *apple-darwin) name="$name.app.tar.gz" ;;
    *) name="$name.AppImage" ;;
  esac
  base="https://github.com/gu1p/nodeharbor/releases/download/v${version}"
  NODEHARBOR_TEMP_WORK=$(mktemp -d "${TMPDIR:-/tmp}/nodeharbor-install.XXXXXX")
  trap nodeharbor_cleanup EXIT
  printf 'Downloading NodeHarbor %s for %s…\n' "$version" "$target"
  nodeharbor_download "$base/SHA256SUMS" "$NODEHARBOR_TEMP_WORK/SHA256SUMS"
  nodeharbor_download "$base/$name" "$NODEHARBOR_TEMP_WORK/$name"
  expected=$(awk -v name="$name" '$2 == name { count++; hash=$1 } END { if(count==1) print hash }' "$NODEHARBOR_TEMP_WORK/SHA256SUMS")
  actual=$(nodeharbor_hash "$NODEHARBOR_TEMP_WORK/$name")
  [[ "$expected" =~ ^[0-9a-f]{64}$ && "$actual" == "$expected" ]] || nodeharbor_error 'Package checksum verification failed; the previous installation has been preserved'
  case "$target" in
    *apple-darwin)
      install_dir=${NODEHARBOR_INSTALL_DIR:-"$HOME/Applications"}
      mkdir -p "$install_dir"
      stage=$(mktemp -d "$install_dir/.nodeharbor-stage.XXXXXX")
      # The verified release contains one application bundle.
      tar -xzf "$NODEHARBOR_TEMP_WORK/$name" -C "$stage"
      binary="$stage/NodeHarbor.app/Contents/MacOS/nodeharbor"
      [[ -x "$binary" ]] || nodeharbor_error 'The package has no runnable application'
      "$binary" --version | grep -F "NodeHarbor $version " >/dev/null || nodeharbor_error 'The downloaded application has the wrong version'
      NODEHARBOR_DESTINATION="$install_dir/NodeHarbor.app"
      agent="$NODEHARBOR_DESTINATION/Contents/MacOS/nodeharbor-agent"
      if [[ -e "$NODEHARBOR_DESTINATION" ]]; then
        [[ -x "$agent" ]] || nodeharbor_error 'The existing app has no drain helper; quit it and move it aside before installing'
        "$agent" prepare-update
        "$NODEHARBOR_DESTINATION/Contents/MacOS/nodeharbor" --quit
        NODEHARBOR_BACKUP="$install_dir/.NodeHarbor.previous.$(date +%s).$$.app"
        mv "$NODEHARBOR_DESTINATION" "$NODEHARBOR_BACKUP"
      fi
      mv "$stage/NodeHarbor.app" "$NODEHARBOR_DESTINATION"
      rmdir "$stage"
      printf 'Installed %s\n' "$NODEHARBOR_DESTINATION"
      ;;
    *)
      install_dir=${NODEHARBOR_INSTALL_DIR:-"$HOME/.local/share/nodeharbor"}
      mkdir -p "$install_dir/releases" "$HOME/.local/bin" "$HOME/.local/share/applications"
      chmod +x "$NODEHARBOR_TEMP_WORK/$name"
      (cd "$NODEHARBOR_TEMP_WORK" && "./$name" --appimage-extract >/dev/null)
      stage="$NODEHARBOR_TEMP_WORK/squashfs-root"
      [[ -x "$stage/AppRun" ]] || nodeharbor_error 'The package has no runnable application'
      "$stage/AppRun" --version | grep -F "NodeHarbor $version " >/dev/null || nodeharbor_error 'The downloaded application has the wrong version'
      if [[ -e "$install_dir/current" ]]; then
        agent="$install_dir/current/usr/bin/nodeharbor-agent"
        [[ -x "$agent" ]] || nodeharbor_error 'The existing app has no drain helper; quit it and move it aside before installing'
        "$agent" prepare-update
      fi
      # Every extracted version has its own directory; replacing the symlink
      # is atomic and keeps the previous version available for recovery.
      local release_dir
      release_dir=$(mktemp -d "$install_dir/releases/v${version}.XXXXXX")
      cp -a "$stage/." "$release_dir/"
      ln -s "$release_dir" "$install_dir/.current.$$"
      mv -Tf "$install_dir/.current.$$" "$install_dir/current"
      ln -sfn "$install_dir/current/AppRun" "$HOME/.local/bin/nodeharbor"
      printf '[Desktop Entry]\nType=Application\nName=NodeHarbor\nExec="%s/current/AppRun"\nIcon=%s/current/nodeharbor.png\nCategories=Development;System;\nTerminal=false\n' "$install_dir" "$install_dir" > "$HOME/.local/share/applications/nodeharbor.desktop"
      printf 'Installed %s. Launch from your application menu or ~/.local/bin/nodeharbor.\n' "$install_dir"
      ;;
  esac
  printf 'Enrollment and resource limits were preserved. Sharing stays paused after an update.\n'
  if [[ -n "$NODEHARBOR_BACKUP" ]]; then printf 'Previous application retained at %s\n' "$NODEHARBOR_BACKUP"; fi
}

if [[ -z "${BASH_SOURCE[0]:-}" || "${BASH_SOURCE[0]}" == "$0" ]]; then nodeharbor_main "$@"; fi
