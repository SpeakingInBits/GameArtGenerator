#!/usr/bin/env bash
# Builds the app and packages it with Velopack, so installed copies can update themselves from the GitHub releases.
#
#   VERSION=1.2.3 packaging/package.sh
#
# Run from the repository root on Windows (Git Bash), with Rust, Node and vpk on the PATH. vpk must be the same
# version as the velopack crate in src-tauri/Cargo.toml (dotnet tool install -g vpk --version <that version>).
# The runtime (win-x64) is also the Velopack channel, so an installed app only updates to its own platform's package.
# Writes to dist/:
#
#   GameArtGenerator-win-x64-Setup.exe       installer (installs for the current user, no admin needed)
#   GameArtGenerator-win-x64-Portable.zip    a folder that runs from anywhere, keeps its data with it, and still updates
#   GameArtGenerator-<version>-win-x64-full.nupkg, releases.win-x64.json   what installed apps download to update
set -euo pipefail

version=${VERSION:?set VERSION, e.g. VERSION=1.2.3}
rid=win-x64
mkdir -p dist
dist=$(cd dist && pwd)

npm ci --no-fund --no-audit

# The version goes into the exe's file properties. (The app shows the version Velopack installed.)
npx tauri build --no-bundle --config "{\"version\":\"$version\"}"

app=out/$rid/GameArtGenerator
rm -rf "out/$rid"
mkdir -p "$app"
cp src-tauri/target/release/GameArtGenerator.exe "$app/"

# No delta packages: each update downloads the whole app, which keeps the release simple.
# --framework webview2: Setup installs Microsoft's WebView2 runtime if the PC doesn't have it (Windows 11 always does).
vpk pack --packId GameArtGenerator --packTitle "Game Art Generator" --packAuthors SpeakingInBits \
  --packVersion "$version" --channel "$rid" --runtime "$rid" --delta None --outputDir "$dist" \
  --packDir "$app" --mainExe GameArtGenerator.exe --icon src-tauri/icons/icon.ico --framework webview2
