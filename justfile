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

# Build a macOS DMG in dist/ (override DIST_DIR to change the output root).
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
    bundle_id="${AXIS_MACOS_BUNDLE_ID:-tech.artelproject.axis}"
    team_id="${AXIS_MACOS_TEAM_ID:-6DR98YW3PY}"
    requested_sign_identity="${AXIS_MACOS_SIGN_IDENTITY:-}"
    notary_profile="${AXIS_MACOS_NOTARY_PROFILE:-}"
    sign_identity="$requested_sign_identity"
    signing_mode="adhoc"

    if [[ -z "$sign_identity" ]] && command -v security >/dev/null 2>&1; then
      identity_matches=()
      while IFS= read -r line; do
        identity_matches+=("$line")
      done < <(security find-identity -v -p codesigning 2>/dev/null | sed -n 's/.*"\(Developer ID Application: [^"]*\)".*/\1/p')

      if [[ ${#identity_matches[@]} -eq 1 ]]; then
        sign_identity="${identity_matches[0]}"
        signing_mode="developer-id"
      elif [[ ${#identity_matches[@]} -gt 1 ]]; then
        echo "Multiple Developer ID Application identities found; set AXIS_MACOS_SIGN_IDENTITY to choose one." >&2
      else
        identity_matches=()
        while IFS= read -r line; do
          identity_matches+=("$line")
        done < <(security find-identity -v -p codesigning 2>/dev/null | sed -n 's/.*"\(Apple Development: [^"]*\)".*/\1/p')

        if [[ ${#identity_matches[@]} -eq 1 ]]; then
          sign_identity="${identity_matches[0]}"
          signing_mode="apple-development"
        elif [[ ${#identity_matches[@]} -gt 1 ]]; then
          echo "Multiple Apple Development identities found; set AXIS_MACOS_SIGN_IDENTITY to choose one." >&2
        fi
      fi
    fi

    if [[ -n "$requested_sign_identity" ]]; then
      case "$requested_sign_identity" in
        Developer\ ID\ Application:*) signing_mode="developer-id" ;;
        Apple\ Development:*) signing_mode="apple-development" ;;
        *) signing_mode="custom" ;;
      esac
    fi

    sign_target() {
      local target="$1"

      if [[ -n "$sign_identity" ]]; then
        if [[ "$signing_mode" == "developer-id" || "$signing_mode" == "apple-development" || "$signing_mode" == "custom" ]]; then
          codesign --force --options runtime --timestamp --sign "$sign_identity" "$target" >/dev/null
        else
          codesign --force --sign "$sign_identity" "$target" >/dev/null
        fi
      else
        codesign --force --sign - "$target" >/dev/null
      fi
    }

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
        <string>${bundle_id}</string>
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

    if [[ -n "$sign_identity" ]]; then
      echo "Signing Axis.app with ${sign_identity} (team ${team_id}, bundle ${bundle_id})"
      if [[ "$signing_mode" == "apple-development" ]]; then
        echo "Apple Development signing is suitable for local installs, but notarization will be skipped." >&2
      fi
    else
      echo "No Developer ID Application identity resolved; falling back to ad-hoc signing." >&2
    fi

    sign_target "$macos_contents_dir/libghostty-vt.dylib"
    sign_target "$macos_contents_dir/axisd"
    sign_target "$macos_contents_dir/axis"
    sign_target "$macos_contents_dir/axis-app"
    sign_target "$app_bundle"

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

    if [[ -n "$sign_identity" ]]; then
      sign_target "$dmg_path"
      codesign --verify --verbose=2 "$dmg_path" >/dev/null
    fi

    if [[ -n "$notary_profile" ]]; then
      if [[ -z "$sign_identity" ]]; then
        echo "AXIS_MACOS_NOTARY_PROFILE requires a Developer ID Application signature." >&2
        exit 1
      fi

      if [[ "$signing_mode" != "developer-id" ]]; then
        echo "Notarization requires a Developer ID Application certificate; skipping submit for signing mode ${signing_mode}." >&2
        echo "Created $dmg_path"
        exit 0
      fi

      if ! xcrun notarytool history --keychain-profile "$notary_profile" >/dev/null 2>&1; then
        echo "Notary profile ${notary_profile} is not configured; skipping notarization." >&2
        echo "Run: xcrun notarytool store-credentials \"$notary_profile\" --apple-id YOUR_APPLE_ID --team-id ${team_id} --password APP_SPECIFIC_PASSWORD" >&2
        echo "Created $dmg_path"
        exit 0
      fi

      echo "Submitting ${dmg_path} for notarization with profile ${notary_profile} (team ${team_id})"
      xcrun notarytool submit "$dmg_path" --keychain-profile "$notary_profile" --team-id "$team_id" --wait >/dev/null
      xcrun stapler staple "$dmg_path" >/dev/null
    fi

    echo "Created $dmg_path"

fmt:
    cargo fmt --all
