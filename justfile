default:
    @just --list

doctor:
    @bash scripts/bootstrap-macos.sh --doctor

check:
    cargo check --workspace

test:
    cargo test -q

clippy:
    cargo clippy --workspace --all-targets

run:
    cargo run -p axis-app

smoke-acp:
    bash scripts/smoke-acp-demo.sh

# Build a macOS dev DMG in dist/ (override DIST_DIR to change the output root).
dmg:
    #!/usr/bin/env bash
    set -euo pipefail

    if [[ "$(uname -s)" != "Darwin" ]]; then
      echo "The dmg recipe only works on macOS." >&2
      exit 1
    fi

    version="$(python3 -c 'import pathlib, tomllib; print(tomllib.loads(pathlib.Path("Cargo.toml").read_text(encoding="utf-8"))["workspace"]["package"]["version"])')"
    dist_dir="${DIST_DIR:-dist}"
    target_dir="${CARGO_TARGET_DIR:-target}"
    release_dir="$target_dir/release"
    macos_dir="$dist_dir/macos"
    app_bundle="$macos_dir/Axis.app"
    contents_dir="$app_bundle/Contents"
    macos_contents_dir="$contents_dir/MacOS"
    resources_dir="$contents_dir/Resources"
    iconset_dir="$macos_dir/Axis.iconset"
    dmg_stage_dir="$macos_dir/dmg-stage"
    dmg_path="$dist_dir/Axis-$version.dmg"
    icon_source="assets/branding/previews/axis-icon.svg.png"

    cargo build --release -p axis-app -p axisd -p axis-cli

    rm -rf "$app_bundle" "$iconset_dir" "$dmg_stage_dir"
    rm -f "$dmg_path"

    mkdir -p "$macos_contents_dir" "$resources_dir" "$iconset_dir" "$dmg_stage_dir"

    install -m 755 "$release_dir/axis-app" "$macos_contents_dir/axis-app"
    install -m 755 "$release_dir/axisd" "$macos_contents_dir/axisd"
    install -m 755 "$release_dir/axis" "$macos_contents_dir/axis"
    install -m 755 "$release_dir/libghostty-vt.dylib" "$macos_contents_dir/libghostty-vt.dylib"

    for size in 16 32 128 256 512; do
      sips -z "$size" "$size" "$icon_source" --out "$iconset_dir/icon_${size}x${size}.png" >/dev/null
      retina_size=$((size * 2))
      sips -z "$retina_size" "$retina_size" "$icon_source" --out "$iconset_dir/icon_${size}x${size}@2x.png" >/dev/null
    done
    iconutil -c icns "$iconset_dir" -o "$resources_dir/Axis.icns"

    cat > "$contents_dir/Info.plist" <<EOF
    <?xml version="1.0" encoding="UTF-8"?>
    <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
    <plist version="1.0">
    <dict>
        <key>CFBundleDevelopmentRegion</key>
        <string>en</string>
        <key>CFBundleDisplayName</key>
        <string>Axis</string>
        <key>CFBundleExecutable</key>
        <string>axis-app</string>
        <key>CFBundleIconFile</key>
        <string>Axis</string>
        <key>CFBundleIdentifier</key>
        <string>com.axis.app</string>
        <key>CFBundleInfoDictionaryVersion</key>
        <string>6.0</string>
        <key>CFBundleName</key>
        <string>Axis</string>
        <key>CFBundlePackageType</key>
        <string>APPL</string>
        <key>CFBundleShortVersionString</key>
        <string>${version}</string>
        <key>CFBundleVersion</key>
        <string>${version}</string>
        <key>LSMinimumSystemVersion</key>
        <string>10.15.7</string>
        <key>NSHighResolutionCapable</key>
        <true/>
    </dict>
    </plist>
    EOF

    plutil -lint "$contents_dir/Info.plist" >/dev/null
    codesign --force --deep --sign - "$app_bundle" >/dev/null
    codesign --verify --deep --strict "$app_bundle"

    ditto "$app_bundle" "$dmg_stage_dir/Axis.app"
    ln -s /Applications "$dmg_stage_dir/Applications"

    hdiutil create \
      -volname "Axis" \
      -srcfolder "$dmg_stage_dir" \
      -ov \
      -format UDZO \
      "$dmg_path" >/dev/null
    hdiutil verify "$dmg_path" >/dev/null

    echo "Created $dmg_path"

fmt:
    cargo fmt --all
