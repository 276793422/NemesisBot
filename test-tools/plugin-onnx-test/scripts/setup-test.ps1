# setup-test.ps1 — Download all-MiniLM-L6-v2 ONNX model + tokenizer for testing
#
# Usage:
#   powershell -File test-tools/plugin-onnx-test/scripts/setup-test.ps1
#
# Downloads model.onnx and tokenizer.json to test-tools/plugin-onnx-test/test-data/

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjectRoot = Resolve-Path (Join-Path $ScriptDir "..\..\..")
$TestDataDir = Join-Path $ProjectRoot "test-tools\plugin-onnx-test\test-data"

$ModelName = "sentence-transformers/all-MiniLM-L6-v2"
$BaseUrl = "https://hf-mirror.com/$ModelName/resolve/main"

Write-Host "[setup-test] Downloading test model: $ModelName"
Write-Host "[setup-test] Target: $TestDataDir"
Write-Host ""

New-Item -ItemType Directory -Force -Path $TestDataDir | Out-Null

# Download model.onnx
$ModelFile = Join-Path $TestDataDir "model.onnx"
if (Test-Path $ModelFile) {
    Write-Host "[setup-test] model.onnx already exists, skipping download"
} else {
    Write-Host "[setup-test] Downloading model.onnx (~90 MB)..."
    Invoke-WebRequest -Uri "$BaseUrl/onnx/model.onnx" -OutFile $ModelFile
    Write-Host "[setup-test] OK model.onnx downloaded"
}

# Download tokenizer.json
$TokenizerFile = Join-Path $TestDataDir "tokenizer.json"
if (Test-Path $TokenizerFile) {
    Write-Host "[setup-test] tokenizer.json already exists, skipping download"
} else {
    Write-Host "[setup-test] Downloading tokenizer.json (~470 KB)..."
    Invoke-WebRequest -Uri "$BaseUrl/tokenizer.json" -OutFile $TokenizerFile
    Write-Host "[setup-test] OK tokenizer.json downloaded"
}

# Verify
$ModelSize = (Get-Item $ModelFile).Length
$TokSize = (Get-Item $TokenizerFile).Length
Write-Host ""
Write-Host "[setup-test] Verification:"
Write-Host "  model.onnx:      $([math]::Round($ModelSize / 1MB)) MB"
Write-Host "  tokenizer.json:  $([math]::Round($TokSize / 1KB)) KB"

if ($ModelSize -lt 1MB) {
    Write-Host "[setup-test] ERROR: model.onnx is too small, download may have failed"
    exit 1
}
if ($TokSize -lt 1KB) {
    Write-Host "[setup-test] ERROR: tokenizer.json is too small, download may have failed"
    exit 1
}

Write-Host ""
Write-Host "[setup-test] Ready. Run tests with:"
Write-Host "  cd plugins/plugin-onnx"
Write-Host '  $env:PLUGIN_ONNX_TEST_MODEL_DIR = "PATH_TO\test-tools\plugin-onnx-test\test-data"'
Write-Host "  cargo test -- --ignored"
