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

# Use the platform's JSON parser. macOS includes plutil; Ubuntu includes Python 3.
nodeharbor_json_value() {
  if [[ -x /usr/bin/plutil ]]; then
    /usr/bin/plutil -extract "$2" raw -expect "$3" -o - "$1"
  else
    command -v python3 >/dev/null || { nodeharbor_error 'Python 3 is required to read release metadata on Linux'; return 1; }
    python3 -c '
import json, sys
with open(sys.argv[1]) as file: value = json.load(file)
for key in sys.argv[2].split("."):
    value = value[int(key)] if isinstance(value, list) else value[key]
kind = sys.argv[3]
if type(value) is not {"string": str, "bool": bool, "array": list}[kind]:
    raise ValueError("Invalid release metadata type")
print(len(value) if kind == "array" else (str(value).lower() if kind == "bool" else value))
' "$1" "$2" "$3"
  fi
}

nodeharbor_release_digest() {
  local metadata=$1 version=$2 name=$3 count index candidate digest='' matches=0
  [[ $(nodeharbor_json_value "$metadata" tag_name string) == "v$version" && $(nodeharbor_json_value "$metadata" draft bool) == false ]] || { nodeharbor_error 'The requested release is not published'; return 1; }
  count=$(nodeharbor_json_value "$metadata" assets array) || return 1
  [[ "$count" =~ ^[0-9]+$ && "$count" -le 1000 ]] || { nodeharbor_error 'Invalid release asset count'; return 1; }
  for ((index=0; index<count; index++)); do
    candidate=$(nodeharbor_json_value "$metadata" "assets.$index.name" string) || return 1
    if [[ "$candidate" == "$name" ]]; then
      matches=$((matches+1))
      [[ $(nodeharbor_json_value "$metadata" "assets.$index.state" string) == uploaded && $(nodeharbor_json_value "$metadata" "assets.$index.browser_download_url" string) == "https://github.com/gu1p/nodeharbor/releases/download/v$version/$name" ]] || { nodeharbor_error 'Invalid release package metadata'; return 1; }
      digest=$(nodeharbor_json_value "$metadata" "assets.$index.digest" string) || return 1
    fi
  done
  [[ "$matches" == 1 && "$digest" =~ ^sha256:[0-9a-f]{64}$ ]] || { nodeharbor_error 'Release package checksum metadata is missing or invalid'; return 1; }
  printf '%s\n' "${digest#sha256:}"
}

nodeharbor_dmg_mountpoint() {
  local metadata=$1 count index candidate mount='' matches=0
  count=$(nodeharbor_json_value "$metadata" system-entities array) || return 1
  for ((index=0; index<count; index++)); do
    candidate=$(nodeharbor_json_value "$metadata" "system-entities.$index.mount-point" string 2>/dev/null) || continue
    matches=$((matches+1))
    mount=$candidate
  done
  [[ "$matches" == 1 && -d "$mount/NodeHarbor.app" ]] || { nodeharbor_error 'The disk image must contain one mounted NodeHarbor application'; return 1; }
  printf '%s\n' "$mount"
}

NODEHARBOR_TEMP_WORK=''
NODEHARBOR_BACKUP=''
NODEHARBOR_DESTINATION=''
NODEHARBOR_STAGE=''
NODEHARBOR_MOUNT=''
nodeharbor_cleanup() {
  if [[ -n "$NODEHARBOR_MOUNT" ]]; then
    hdiutil detach "$NODEHARBOR_MOUNT" >/dev/null || { nodeharbor_error "Could not detach the installer disk image at $NODEHARBOR_MOUNT; temporary files have been retained"; return 1; }
  fi
  if [[ -n "$NODEHARBOR_BACKUP" && -e "$NODEHARBOR_BACKUP" && ! -e "$NODEHARBOR_DESTINATION" ]]; then
    mv "$NODEHARBOR_BACKUP" "$NODEHARBOR_DESTINATION"
  fi
  if [[ -n "$NODEHARBOR_TEMP_WORK" ]]; then rm -rf -- "$NODEHARBOR_TEMP_WORK"; fi
  if [[ -n "$NODEHARBOR_STAGE" ]]; then rm -rf -- "$NODEHARBOR_STAGE"; fi
}

nodeharbor_close_application() {
  local agent=$1 launcher=$2 executable=$3
  "$agent" prepare-update
  "$launcher" --quit
  "$agent" wait-for-app-exit --executable "$executable" --timeout 30
}

nodeharbor_activation_cleanup() {
    local status=$1 committed=$2 work=$3 bin_stage=$4 menu_stage=$5 launcher=$6 entry=$7 bin_started=$8 menu_started=$9
    local failed=0 directory
    if [[ $committed == 0 ]]; then
      if [[ $menu_started == 1 ]]; then
        if [[ -e "$menu_stage/previous" || -L "$menu_stage/previous" ]]; then
          mv -Tf -- "$menu_stage/previous" "$entry" || failed=1
        else rm -f -- "$entry" || failed=1; fi
      fi
      if [[ $bin_started == 1 ]]; then
        if [[ -e "$bin_stage/previous" || -L "$bin_stage/previous" ]]; then
          mv -Tf -- "$bin_stage/previous" "$launcher" || failed=1
        else rm -f -- "$launcher" || failed=1; fi
      fi
    fi
    if [[ $failed == 1 ]]; then
      printf 'NodeHarbor: launcher restoration failed; previous files remain in %s and %s\n' "$bin_stage" "$menu_stage" >&2
      return 1
    fi
    for directory in "$work" "$bin_stage" "$menu_stage"; do
      if [[ -n "$directory" ]]; then rm -rf -- "$directory"; fi
    done
    return "$status"
  }

# Publish the Linux version last. Each rename stays on its target filesystem;
# an interrupted update restores the previous launcher and desktop entry.
nodeharbor_activate_linux() (
  local install_dir=$1 release_dir=$2 launcher=$3 entry=$4
  local work='' bin_stage='' menu_stage='' committed=0 bin_started=0 menu_started=0
  [[ ! -e "$install_dir/current" || -L "$install_dir/current" ]] || nodeharbor_error 'The current installation is not a managed version link'
  [[ ! -e "$launcher" || -L "$launcher" ]] || nodeharbor_error 'The application launcher is not a managed symbolic link'
  [[ ! -e "$entry" || -f "$entry" ]] || nodeharbor_error 'The application menu entry is not a regular file'
  exec 9>"$install_dir/.install.lock"
  flock -n 9 || nodeharbor_error 'Another NodeHarbor installation is in progress'
  trap 'nodeharbor_activation_cleanup "$?" "$committed" "$work" "$bin_stage" "$menu_stage" "$launcher" "$entry" "$bin_started" "$menu_started"' EXIT
  trap 'exit 130' INT TERM HUP
  work=$(mktemp -d "$install_dir/.nodeharbor-activation.XXXXXX")
  bin_stage=$(mktemp -d "$(dirname "$launcher")/.nodeharbor-activation.XXXXXX")
  menu_stage=$(mktemp -d "$(dirname "$entry")/.nodeharbor-activation.XXXXXX")
  if [[ -e "$launcher" || -L "$launcher" ]]; then cp -a -- "$launcher" "$bin_stage/previous"; fi
  if [[ -e "$entry" || -L "$entry" ]]; then cp -a -- "$entry" "$menu_stage/previous"; fi
  ln -s -- "$release_dir" "$work/current"
  ln -s -- "$install_dir/current/AppRun" "$bin_stage/next"
  printf '[Desktop Entry]\nType=Application\nName=NodeHarbor\nExec="%s/current/AppRun"\nIcon=%s/current/nodeharbor.png\nCategories=Development;System;\nTerminal=false\n' "$install_dir" "$install_dir" > "$menu_stage/next"
  chmod 644 "$menu_stage/next"
  bin_started=1
  mv -Tf -- "$bin_stage/next" "$launcher"
  menu_started=1
  mv -Tf -- "$menu_stage/next" "$entry"
  mv -Tf -- "$work/current" "$install_dir/current"
  committed=1
)

nodeharbor_main() {
  local target version name base expected actual install_dir stage binary agent mount
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
    *apple-darwin) name="$name.dmg" ;;
    *) name="$name.AppImage" ;;
  esac
  base="https://github.com/gu1p/nodeharbor/releases/download/v${version}"
  NODEHARBOR_TEMP_WORK=$(mktemp -d "${TMPDIR:-/tmp}/nodeharbor-install.XXXXXX")
  trap nodeharbor_cleanup EXIT
  printf 'Downloading NodeHarbor %s for %s…\n' "$version" "$target"
  nodeharbor_download "https://api.github.com/repos/gu1p/nodeharbor/releases/tags/v$version" "$NODEHARBOR_TEMP_WORK/release.json"
  expected=$(nodeharbor_release_digest "$NODEHARBOR_TEMP_WORK/release.json" "$version" "$name")
  nodeharbor_download "$base/$name" "$NODEHARBOR_TEMP_WORK/$name"
  actual=$(nodeharbor_hash "$NODEHARBOR_TEMP_WORK/$name")
  [[ "$expected" =~ ^[0-9a-f]{64}$ && "$actual" == "$expected" ]] || nodeharbor_error 'Package checksum verification failed; the previous installation has been preserved'
  case "$target" in
    *apple-darwin)
      install_dir=${NODEHARBOR_INSTALL_DIR:-"$HOME/Applications"}
      mkdir -p "$install_dir"
      stage=$(mktemp -d "$install_dir/.nodeharbor-stage.XXXXXX")
      NODEHARBOR_STAGE="$stage"
      hdiutil attach "$NODEHARBOR_TEMP_WORK/$name" -nobrowse -readonly -plist > "$NODEHARBOR_TEMP_WORK/mount.plist"
      NODEHARBOR_MOUNT=$(nodeharbor_json_value "$NODEHARBOR_TEMP_WORK/mount.plist" system-entities.0.dev-entry string)
      mount=$(nodeharbor_dmg_mountpoint "$NODEHARBOR_TEMP_WORK/mount.plist")
      ditto "$mount/NodeHarbor.app" "$stage/NodeHarbor.app"
      hdiutil detach "$NODEHARBOR_MOUNT" >/dev/null
      NODEHARBOR_MOUNT=''
      binary="$stage/NodeHarbor.app/Contents/MacOS/nodeharbor"
      [[ -x "$binary" ]] || nodeharbor_error 'The package has no runnable application'
      "$binary" --version | grep -F "NodeHarbor $version " >/dev/null || nodeharbor_error 'The downloaded application has the wrong version'
      NODEHARBOR_DESTINATION="$install_dir/NodeHarbor.app"
      agent="$NODEHARBOR_DESTINATION/Contents/MacOS/nodeharbor-agent"
      if [[ -e "$NODEHARBOR_DESTINATION" ]]; then
        [[ -x "$agent" ]] || nodeharbor_error 'The existing app has no drain helper; quit it and move it aside before installing'
        nodeharbor_close_application "$agent" "$NODEHARBOR_DESTINATION/Contents/MacOS/nodeharbor" "$NODEHARBOR_DESTINATION/Contents/MacOS/nodeharbor"
        NODEHARBOR_BACKUP="$install_dir/.NodeHarbor.previous.$(date +%s).$$.app"
        mv "$NODEHARBOR_DESTINATION" "$NODEHARBOR_BACKUP"
      fi
      mv "$stage/NodeHarbor.app" "$NODEHARBOR_DESTINATION"
      rmdir "$stage"
      NODEHARBOR_STAGE=''
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
        nodeharbor_close_application "$agent" "$install_dir/current/AppRun" "$install_dir/current/usr/bin/nodeharbor"
      fi
      # Every extracted version has its own directory; replacing the symlink
      # is atomic and keeps the previous version available for recovery.
      local release_dir
      release_dir=$(mktemp -d "$install_dir/releases/v${version}.XXXXXX")
      cp -a "$stage/." "$release_dir/"
      nodeharbor_activate_linux "$install_dir" "$release_dir" "$HOME/.local/bin/nodeharbor" "$HOME/.local/share/applications/nodeharbor.desktop"
      printf 'Installed %s. Launch from your application menu or ~/.local/bin/nodeharbor.\n' "$install_dir"
      ;;
  esac
  printf 'Enrollment and resource limits were preserved. Sharing stays paused after an update.\n'
  if [[ -n "$NODEHARBOR_BACKUP" ]]; then printf 'Previous application retained at %s\n' "$NODEHARBOR_BACKUP"; fi
}

if [[ -z "${BASH_SOURCE[0]:-}" || "${BASH_SOURCE[0]}" == "$0" ]]; then nodeharbor_main "$@"; fi
