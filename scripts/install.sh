#!/usr/bin/env bash
set -euo pipefail

if [[ ${1:-} == --help && $# == 1 ]]; then
    printf '%s\n' 'Usage: ./scripts/install.sh' \
        'Build and install UClip for the current Linux user, then launch it.' \
        'A running copy of this installation is stopped after the build succeeds.'
    exit 0
fi

fail() {
    printf 'UClip install: %s\n' "$1" >&2
    exit 1
}

[[ $# == 0 ]] || fail 'Run with --help for usage.'
[[ $(uname -s) == Linux ]] || fail 'This installer requires Linux.'
[[ $EUID != 0 ]] || fail 'Run as your desktop user without sudo.'

cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
for tool in node npm cargo rustc; do
    command -v "$tool" >/dev/null || fail "Missing $tool. See the build prerequisites in README.md."
done

binary="$HOME/.local/bin/uclip-desktop"
data_home=${XDG_DATA_HOME:-"$HOME/.local/share"}
[[ $data_home == /* ]] || data_home="$HOME/.local/share"
launcher="$data_home/applications/app.uclip.desktop"
icon="$data_home/icons/hicolor/256x256/apps/app.uclip.png"

# Exec has a desktop-entry escaping pass before its argument quoting pass.
[[ $binary == /* && $binary != *[[:cntrl:]=]* ]] || fail 'HOME contains an unsupported path.'
exec_path=${binary//\\/\\\\\\\\}
exec_path=${exec_path//\"/\\\\\"}
exec_path=${exec_path//\$/\\\\\$}
exec_path=${exec_path//\`/\\\\\`}
exec_path=${exec_path//%/%%}

mkdir -p -- "${binary%/*}"
installed_executable="$(readlink -f -- "${binary%/*}")/${binary##*/}"
exec {install_lock}>>"$binary.install.lock"
flock -n "$install_lock" || fail 'Another UClip installation is already running.'

is_installed_process() {
    local process="/proc/$1" executable
    [[ -O $process ]] || return 1
    executable=$(readlink -- "$process/exe" 2>/dev/null) || return 1
    # A previous install may have replaced the file while this copy was running.
    [[ $executable == "$installed_executable" || $executable == "$installed_executable (deleted)" ]]
}

stop_installed_app() {
    local process pid deadline
    local -a stopping=()
    for process in /proc/[0-9]*; do
        pid=${process##*/}
        if is_installed_process "$pid"; then
            if ! kill -TERM "$pid" && is_installed_process "$pid"; then
                fail "Could not stop UClip (PID $pid)."
            fi
            stopping+=("$pid")
        fi
    done

    deadline=$((SECONDS + 10))
    for pid in "${stopping[@]}"; do
        while is_installed_process "$pid"; do
            (( SECONDS < deadline )) || fail "UClip (PID $pid) did not exit. Close it and rerun the installer."
            sleep 0.1
        done
    done
}

if [[ ! -f node_modules/@tauri-apps/cli/tauri.js ]]; then
    npm ci
fi

target=$(rustc --print host-tuple)
target_dir=$(cargo metadata --no-deps --format-version 1 --locked |
    node -p 'JSON.parse(require("node:fs").readFileSync(0, "utf8")).target_directory')
npm run tauri:build -- --ci --target "$target" -- --locked
built_binary="$target_dir/$target/release/uclip-desktop"
[[ -f $built_binary && -x $built_binary ]] || fail "Build output missing: $built_binary"

mkdir -p -- "${launcher%/*}" "${icon%/*}"
temporary_files=()
trap 'rm -f -- "${temporary_files[@]}"' EXIT

staged_binary=$(mktemp -- "$binary.XXXXXX")
temporary_files+=("$staged_binary")
staged_launcher=$(mktemp -- "$launcher.XXXXXX")
temporary_files+=("$staged_launcher")
staged_icon=$(mktemp -- "$icon.XXXXXX")
temporary_files+=("$staged_icon")

install -m 755 -- "$built_binary" "$staged_binary"
install -m 644 -- src-tauri/icons/128x128@2x.png "$staged_icon"
# Pass the path as an argument so launchers expand literal percent signs.
cat > "$staged_launcher" <<EOF
[Desktop Entry]
Type=Application
Name=UClip
Comment=Local clipboard manager
Exec=/usr/bin/env -- "$exec_path"
Icon=app.uclip
Terminal=false
Categories=Utility;
EOF
chmod 644 -- "$staged_launcher"

stop_installed_app

# Publish the menu entry only after its executable and icon are in place.
mv -fT -- "$staged_binary" "$binary"
mv -fT -- "$staged_icon" "$icon"
mv -fT -- "$staged_launcher" "$launcher"

# Keep the application running after the installer's terminal session ends.
setsid -- "$binary" </dev/null >/dev/null 2>&1 {install_lock}>&- &
started_pid=$!
sleep 1
is_installed_process "$started_pid" || fail "Installed UClip at $binary, but it did not stay running. Run the executable directly to see the error."

printf 'Installed UClip at %s\nStarted UClip (PID %s).\n' "$binary" "$started_pid"
