# Dot-source this before building:  . .\env.ps1
# Uses the shared portable toolchain in ..\.toolchain (Rust GNU + MinGW + Git).
$tc = Join-Path (Split-Path -Parent $PSScriptRoot) ".toolchain"
$env:RUSTUP_HOME = Join-Path $tc "rustup"
$env:CARGO_HOME = Join-Path $tc "cargo"
$env:PATH = (Join-Path $tc "cargo\bin") + ";" + (Join-Path $tc "mingw\mingw64\bin") + ";" + (Join-Path $tc "git\cmd") + ";" + $env:PATH
# Keep build output inside this project.
$env:CARGO_TARGET_DIR = Join-Path $PSScriptRoot "target"
